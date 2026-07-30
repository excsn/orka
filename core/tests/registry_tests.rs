// tests/registry_tests.rs
mod common;

use common::*;
use orka::test_util::MockPipeline;
use orka::{
  CancelToken, ContextData, Orka, OrkaError, Pipeline, PipelineControl, PipelineResult, PlannedAction, RunOutcome,
  SkipReason, TraceCollector, TraceEventKind,
};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegistryContextAlpha {
  val: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegistryContextBeta {
  num: i32,
}

#[tokio::test]
async fn test_registry_run_correct_pipeline() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(["alpha_task"]);
  p_alpha.on_root("alpha_task", |ctx| async move {
    ctx.write().val = "alpha_processed".to_string();
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p_alpha).unwrap();

  let mut p_beta = Pipeline::<RegistryContextBeta, TestError>::new(["beta_task"]);
  p_beta.on_root("beta_task", |ctx| async move {
    ctx.write().num = 100;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p_beta).unwrap();

  // Run Alpha
  let ctx_alpha = ContextData::new(RegistryContextAlpha::default());
  let res_alpha = orka_registry.run(ctx_alpha.clone()).await;
  assert!(res_alpha.is_ok());
  assert_eq!(res_alpha.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx_alpha.read().val, "alpha_processed");

  // Run Beta
  let ctx_beta = ContextData::new(RegistryContextBeta::default());
  let res_beta = orka_registry.run(ctx_beta.clone()).await;
  assert!(res_beta.is_ok());
  assert_eq!(res_beta.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx_beta.read().num, 100);
}

#[tokio::test]
async fn test_registry_pipeline_not_found() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  #[derive(Clone, Debug, Default)]
  struct UnregisteredContext;

  let ctx_unregistered = ContextData::new(UnregisteredContext);
  let result = orka_registry.run(ctx_unregistered).await;

  assert!(result.is_err());
  if let Err(TestError::Orka(s)) = result {
    assert!(s.contains("ConfigurationError"));
    assert!(s.contains("No pipeline registered"));
    assert!(s.contains("UnregisteredContext"));
  } else {
    panic!(
      "Expected OrkaError(ConfigurationError) for unregistered pipeline, got {:?}",
      result
    );
  }
}

#[tokio::test]
async fn test_registry_pipeline_itself_errors() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(["alpha_fail"]);
  p_alpha.on_root("alpha_fail", |_ctx| async move {
    Err(TestError::Handler("Alpha pipeline failed".to_string()))
  });
  orka_registry.register_pipeline(p_alpha).unwrap();

  let ctx_alpha = ContextData::new(RegistryContextAlpha::default());
  let res_alpha = orka_registry.run(ctx_alpha.clone()).await;

  assert!(res_alpha.is_err());
  assert_eq!(
    res_alpha.err().unwrap(),
    TestError::Handler("Alpha pipeline failed".to_string())
  );
}

#[tokio::test]
async fn test_registry_with_orka_error_default() {
  setup_tracing();
  let orka_registry = Orka::<OrkaError>::new_default();

  #[derive(Clone, Debug, Default)]
  struct SimpleCtx {
    count: i32,
  }

  // The pipeline's handler error type must match the registry's application error type.
  let mut pipeline = Pipeline::<SimpleCtx, OrkaError>::new(["task"]);
  pipeline.on_root("task", |ctx| async move {
    ctx.write().count = 1;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(pipeline).unwrap();

  let ctx = ContextData::new(SimpleCtx::default());
  let result = orka_registry.run(ctx.clone()).await;
  assert!(result.is_ok());
  assert_eq!(ctx.read().count, 1);
}

// --- Orka::pipeline() accessor: the registry as the test scope ---

fn build_alpha_registry() -> Orka<TestError> {
  // Stands in for an app's production registration fn: same wiring for tests and prod.
  let orka_registry = Orka::<TestError>::new();
  let mut p = Pipeline::<RegistryContextAlpha, TestError>::new(["prepare", "commit"]);
  p.skip_if("prepare", |ctx| ctx.read().val == "already_prepared");
  p.on_root("prepare", |ctx| async move {
    ctx.write().val.push_str("prepared;");
    Ok(PipelineControl::Continue)
  })
  .on_root("commit", |ctx| async move {
    ctx.write().val.push_str("committed;");
    Ok(PipelineControl::Continue)
  })
  .on_finish(|_ctx, _outcome| async { Ok(()) });
  orka_registry.register_pipeline(p).unwrap();
  orka_registry
}

#[tokio::test]
async fn pipeline_accessor_returns_the_registered_pipeline() {
  setup_tracing();
  let orka_registry = build_alpha_registry();

  let p = orka_registry
    .pipeline::<RegistryContextAlpha, TestError>()
    .expect("registered as a concrete pipeline");
  assert_eq!(p.step_names(), vec!["prepare", "commit"]);

  // Nothing registered for this TData.
  #[derive(Clone, Debug, Default)]
  struct NotRegistered;
  assert!(orka_registry.pipeline::<NotRegistered, TestError>().is_none());
}

#[tokio::test]
async fn pipeline_accessor_returns_none_for_runner_registrations_and_wrong_err() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();
  orka_registry.register_runner::<RegistryContextAlpha, TestError>(Arc::new(MockPipeline::completed()));
  // Runner-only registration: no concrete pipeline to hand back.
  assert!(orka_registry.pipeline::<RegistryContextAlpha, TestError>().is_none());

  // A wrong Err type parameter yields None (failed downcast), never a panic.
  let orka_registry2 = build_alpha_registry();
  assert!(orka_registry2.pipeline::<RegistryContextAlpha, OrkaError>().is_none());
}

#[tokio::test]
async fn observe_and_dry_run_the_registered_pipeline_through_the_front_door() {
  setup_tracing();
  let orka_registry = build_alpha_registry();
  let p = orka_registry.pipeline::<RegistryContextAlpha, TestError>().unwrap();

  // Dry-run the skip matrix against seeded contexts; nothing executes.
  let plan = p.resolve_plan(&ContextData::new(RegistryContextAlpha {
    val: "already_prepared".into(),
  }));
  assert_eq!(plan[0].action, PlannedAction::Skip(SkipReason::SkipCondition { label: None }));
  assert_eq!(plan[1].action, PlannedAction::Run);

  // Attach a tracer to the real registered pipeline, then drive the SAME entry point
  // production uses: orka.run. FinalizerFinished proves the run-level cleanup ring is
  // assertable through the front door.
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  let ctx = ContextData::new(RegistryContextAlpha::default());
  assert_eq!(orka_registry.run(ctx.clone()).await.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx.read().val, "prepared;committed;");

  assert_eq!(trace.completed_steps(), vec!["prepare", "commit"]);
  assert_eq!(trace.last_outcome(), Some(RunOutcome::Completed));
  assert!(
    trace
      .events()
      .iter()
      .any(|e| matches!(e.kind, TraceEventKind::FinalizerFinished { .. })),
    "on_finish ring must be visible through orka.run"
  );

  // Step isolation against the registered shape, still through the accessor.
  let step_ctx = ContextData::new(RegistryContextAlpha::default());
  p.run_step("commit", step_ctx.clone()).await.unwrap();
  assert_eq!(step_ctx.read().val, "committed;");
}

/// The registry boxes the context into `Box<dyn Any + Send>` to erase its type. The token
/// rides inside that context rather than alongside it, so cancellation reaches a
/// registry-driven run with no plumbing in the erasure path.
#[tokio::test]
async fn cancellation_survives_the_registry_type_erasure() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();
  let token = CancelToken::new();

  let canceller = token.clone();
  let mut p = Pipeline::<RegistryContextAlpha, TestError>::new(["first", "second"]);
  p.on_root("first", move |ctx: ContextData<RegistryContextAlpha>| {
    let canceller = canceller.clone();
    async move {
      ctx.write().val = "first".to_string();
      canceller.cancel();
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("second", |ctx: ContextData<RegistryContextAlpha>| async move {
    ctx.write().val = "second".to_string();
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p).unwrap();

  let ctx = ContextData::new(RegistryContextAlpha::default());
  let (result, outcome) = orka_registry.run_with_cancel_and_outcome(ctx.clone(), token).await;

  assert_eq!(result.unwrap(), PipelineResult::Cancelled);
  assert_eq!(outcome, RunOutcome::Cancelled);
  assert_eq!(ctx.read().val, "first", "the second step never ran");
}

#[tokio::test]
async fn a_registry_run_with_an_uncancelled_token_completes_normally() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  let mut p = Pipeline::<RegistryContextBeta, TestError>::new(["task"]);
  p.on_root("task", |ctx: ContextData<RegistryContextBeta>| async move {
    ctx.write().num = 7;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p).unwrap();

  let ctx = ContextData::new(RegistryContextBeta::default());
  let result = orka_registry.run_with_cancel(ctx.clone(), CancelToken::new()).await;

  assert_eq!(result.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx.read().num, 7);
}
