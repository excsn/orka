//! Tests for the `PipelineRunner` run-boundary seam: `MockPipeline` through
//! `Orka::register_runner`, canned FIFO sequences, context inspection, and a middleware
//! runner wrapping a real pipeline.

mod common;

use common::{setup_tracing, TestContext, TestError};
use async_trait::async_trait;
use orka::test_util::{ExecutionCounter, MockPipeline};
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl, PipelineResult, PipelineRunner, RunOutcome};
use std::sync::Arc;

#[tokio::test]
async fn mock_pipeline_through_the_registry_front_door() {
  setup_tracing();
  let orka = Orka::<TestError>::new();
  let mock = Arc::new(MockPipeline::<TestContext, TestError>::completed());
  orka.register_runner(mock.clone() as Arc<dyn PipelineRunner<TestContext, TestError>>);

  // The calling code path is exactly production's: orka.run(ctx).
  let ctx = ContextData::new(TestContext {
    message: "from the handler under test".into(),
    ..TestContext::default()
  });
  let result = orka.run(ctx).await.unwrap();
  assert_eq!(result, PipelineResult::Completed);

  assert_eq!(mock.run_count(), 1);
  assert_eq!(mock.contexts()[0].read().message, "from the handler under test");
}

#[tokio::test]
async fn mock_pipeline_fifo_queue_then_base_behavior() {
  setup_tracing();
  let mut mock = MockPipeline::<TestContext, TestError>::completed();
  mock
    .then_stopped()
    .then_error(|| TestError::Other("second run fails".into()));
  let mock = Arc::new(mock);

  let orka = Orka::<TestError>::new();
  orka.register_runner(mock.clone() as Arc<dyn PipelineRunner<TestContext, TestError>>);

  let ctx = || ContextData::new(TestContext::default());
  assert_eq!(orka.run(ctx()).await.unwrap(), PipelineResult::Stopped);
  assert_eq!(orka.run(ctx()).await.unwrap_err(), TestError::Other("second run fails".into()));
  // Queue drained: base behavior answers from here on.
  assert_eq!(orka.run(ctx()).await.unwrap(), PipelineResult::Completed);
  assert_eq!(orka.run(ctx()).await.unwrap(), PipelineResult::Completed);
  assert_eq!(mock.run_count(), 4);
}

#[tokio::test]
async fn mock_from_fn_can_inspect_and_mutate_the_context() {
  setup_tracing();
  let mock = MockPipeline::<TestContext, TestError>::from_fn(|ctx| {
    let mut guard = ctx.write();
    guard.counter += 100;
    if guard.message == "please stop" {
      Ok(PipelineResult::Stopped)
    } else {
      Ok(PipelineResult::Completed)
    }
  });
  let orka = Orka::<TestError>::new();
  orka.register_runner::<TestContext, TestError>(Arc::new(mock));

  let ctx = ContextData::new(TestContext {
    message: "please stop".into(),
    ..TestContext::default()
  });
  assert_eq!(orka.run(ctx.clone()).await.unwrap(), PipelineResult::Stopped);
  assert_eq!(ctx.read().counter, 100);
}

#[tokio::test]
async fn failing_mock_propagates_through_registry_error_conversion() {
  setup_tracing();
  let orka = Orka::<TestError>::new();
  orka.register_runner::<TestContext, TestError>(Arc::new(MockPipeline::failing(|| {
    TestError::Handler("mock failure".into())
  })));
  let err = orka.run(ContextData::new(TestContext::default())).await.unwrap_err();
  assert_eq!(err, TestError::Handler("mock failure".into()));
}

#[tokio::test]
async fn register_runner_replaces_like_register_pipeline() {
  setup_tracing();
  let orka = Orka::<TestError>::new();

  // Real pipeline first...
  let mut real: Pipeline<TestContext, TestError> = Pipeline::new(["work"]);
  real.on_root("work", |ctx| async move {
    ctx.write().counter += 1;
    Ok(PipelineControl::Continue)
  });
  orka.register_pipeline(real).unwrap();

  // ...then a mock registered over it for the same TData wins.
  orka.register_runner::<TestContext, TestError>(Arc::new(MockPipeline::stopped()));
  let ctx = ContextData::new(TestContext::default());
  assert_eq!(orka.run(ctx.clone()).await.unwrap(), PipelineResult::Stopped);
  assert_eq!(ctx.read().counter, 0, "the real pipeline must not have run");
}

/// A logging/counting middleware runner wrapping an inner runner: the production-shaped
/// use of the seam.
struct CountingMiddleware<TData, Err>
where
  TData: 'static + Send + Sync,
{
  inner: Arc<dyn PipelineRunner<TData, Err>>,
  calls: ExecutionCounter,
}

#[async_trait]
impl<TData, Err> PipelineRunner<TData, Err> for CountingMiddleware<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    self.calls.increment();
    self.inner.run(ctx_data).await
  }
}

#[tokio::test]
async fn middleware_runner_composes_around_a_real_pipeline() {
  setup_tracing();
  let mut real: Pipeline<TestContext, TestError> = Pipeline::new(["work"]);
  real.on_root("work", |ctx| async move {
    ctx.write().counter += 1;
    Ok(PipelineControl::Continue)
  });

  let calls = ExecutionCounter::new();
  let middleware = CountingMiddleware {
    inner: Arc::new(real) as Arc<dyn PipelineRunner<TestContext, TestError>>,
    calls: calls.clone(),
  };

  let orka = Orka::<TestError>::new();
  orka.register_runner::<TestContext, TestError>(Arc::new(middleware));

  let ctx = ContextData::new(TestContext::default());
  assert_eq!(orka.run(ctx.clone()).await.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx.read().counter, 1, "inner pipeline ran");
  assert_eq!(calls.get(), 1, "middleware observed the run");
}

#[tokio::test]
async fn orka_run_with_outcome_attributes_the_step_through_the_registry() {
  setup_tracing();
  let orka = Orka::<TestError>::new();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["prepare", "install"]);
  p.on_root("prepare", |_ctx| async { Ok(PipelineControl::Continue) })
    .on_root("install", |_ctx| async { Err(TestError::Handler("unit refused".into())) });
  orka.register_pipeline(p).unwrap();

  let (result, outcome) = orka.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert!(result.is_err());
  // The failing step's name reaches the caller: this is what a job shell puts into its
  // operator-facing failure event.
  assert!(
    matches!(outcome, RunOutcome::Errored { ref step, ref message } if step == "install" && message.contains("unit refused"))
  );
}

#[tokio::test]
async fn orka_run_with_outcome_default_method_and_not_found_cases() {
  setup_tracing();
  let orka = Orka::<TestError>::new();

  // Nothing registered: Errored { step: "Orka::run" }.
  let (result, outcome) = orka.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert!(result.is_err());
  assert!(matches!(outcome, RunOutcome::Errored { ref step, .. } if step == "Orka::run"));

  // A mock uses PipelineRunner's default method: failure attributed to no step (empty).
  orka.register_runner::<TestContext, TestError>(Arc::new(MockPipeline::failing(|| {
    TestError::Handler("mock".into())
  })));
  let (result, outcome) = orka.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert!(result.is_err());
  assert!(matches!(outcome, RunOutcome::Errored { ref step, .. } if step.is_empty()));

  // And Ok outcomes flow through unchanged.
  orka.register_runner::<TestContext, TestError>(Arc::new(MockPipeline::stopped()));
  let (result, outcome) = orka.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert_eq!(result.unwrap(), PipelineResult::Stopped);
  assert_eq!(outcome, RunOutcome::Stopped);
}
