//! Tests for the `on_finish` run-level finish ring: it fires on every exit of a full
//! `run()`, never for the partial runners or `resolve_plan`, and its error policy is
//! surface-on-Ok / preserve-original-on-Err.

mod common;

use common::{setup_tracing, TestContext, TestError};
use orka::test_util::{assert_run_outcome, ExecutionCounter, PipelineTestExt};
use orka::{ContextData, Pipeline, PipelineControl, PipelineResult, RunOutcome, TraceCollector, TraceEventKind};

fn two_step_pipeline() -> Pipeline<TestContext, TestError> {
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["alpha", "beta"]);
  p.on_root("alpha", |ctx| async move {
    ctx.write().steps_executed.push("alpha".into());
    Ok(PipelineControl::Continue)
  })
  .on_root("beta", |ctx| async move {
    ctx.write().steps_executed.push("beta".into());
    Ok(PipelineControl::Continue)
  });
  p
}

#[tokio::test]
async fn on_finish_fires_on_completed() {
  setup_tracing();
  let mut p = two_step_pipeline();
  let counter = ExecutionCounter::new();
  let seen = ContextData::new(Vec::<RunOutcome>::new());
  let seen_in_handler = seen.clone();
  let c = counter.clone();
  p.on_finish(move |_ctx, outcome| {
    let c = c.clone();
    let seen = seen_in_handler.clone();
    async move {
      c.increment();
      seen.write().push(outcome);
      Ok(())
    }
  });

  let ctx = ContextData::new(TestContext::default());
  let result = p.run(ctx).await.unwrap();
  assert_eq!(result, PipelineResult::Completed);
  assert_eq!(counter.get(), 1);
  assert_eq!(seen.read().as_slice(), &[RunOutcome::Completed]);
}

#[tokio::test]
async fn on_finish_fires_on_stopped() {
  setup_tracing();
  let mut p = two_step_pipeline();
  p.replace_on_root("alpha", |_ctx| async { Ok(PipelineControl::Stop) });
  let seen = ContextData::new(Vec::<RunOutcome>::new());
  let seen_in_handler = seen.clone();
  p.on_finish(move |_ctx, outcome| {
    let seen = seen_in_handler.clone();
    async move {
      seen.write().push(outcome);
      Ok(())
    }
  });

  let result = p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_eq!(result, PipelineResult::Stopped);
  assert_eq!(seen.read().as_slice(), &[RunOutcome::Stopped]);
}

#[tokio::test]
async fn on_finish_fires_on_handler_error_and_preserves_original_error() {
  setup_tracing();
  let mut p = two_step_pipeline();
  p.fail_at("beta", || TestError::Handler("boom".into()));
  let seen = ContextData::new(Vec::<RunOutcome>::new());
  let seen_in_handler = seen.clone();
  p.on_finish(move |_ctx, outcome| {
    let seen = seen_in_handler.clone();
    async move {
      seen.write().push(outcome);
      // A finish-handler failure on an already-failed run must NOT mask the original.
      Err(TestError::Other("finish failure".into()))
    }
  });

  let err = p.run(ContextData::new(TestContext::default())).await.unwrap_err();
  assert_eq!(err, TestError::Handler("boom".into()));
  let first_outcome = seen.read()[0].clone();
  match first_outcome {
    RunOutcome::Errored { step, .. } => assert_eq!(step, "beta"),
    other => panic!("expected Errored outcome, got {:?}", other),
  }
}

#[tokio::test]
async fn on_finish_fires_on_missing_handler_config_error() {
  setup_tracing();
  // "beta" is required but has no handlers: run fails with HandlerMissing; the finish
  // ring must still fire.
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["alpha", "beta"]);
  p.on_root("alpha", |_ctx| async { Ok(PipelineControl::Continue) });
  let counter = ExecutionCounter::new();
  let c = counter.clone();
  p.on_finish(move |_ctx, outcome| {
    let c = c.clone();
    async move {
      assert!(matches!(outcome, RunOutcome::Errored { .. }));
      c.increment();
      Ok(())
    }
  });

  let err = p.run(ContextData::new(TestContext::default())).await.unwrap_err();
  assert!(matches!(err, TestError::Orka(_)));
  assert_eq!(counter.get(), 1);
}

#[tokio::test]
async fn finish_error_on_ok_run_becomes_the_run_error_for_completed_and_stopped() {
  setup_tracing();
  for stop in [false, true] {
    let mut p = two_step_pipeline();
    if stop {
      p.replace_on_root("beta", |_ctx| async { Ok(PipelineControl::Stop) });
    }
    p.on_finish(|_ctx, _outcome| async { Err(TestError::Other("cleanup failed".into())) });

    let err = p.run(ContextData::new(TestContext::default())).await.unwrap_err();
    assert_eq!(err, TestError::Other("cleanup failed".into()), "stop = {}", stop);
  }
}

#[tokio::test]
async fn all_finish_handlers_run_in_order_even_when_one_fails() {
  setup_tracing();
  let mut p = two_step_pipeline();
  let order = ContextData::new(Vec::<&'static str>::new());
  let (o1, o2, o3) = (order.clone(), order.clone(), order.clone());
  p.on_finish(move |_ctx, _outcome| {
    let o = o1.clone();
    async move {
      o.write().push("first");
      Ok(())
    }
  })
  .on_finish(move |_ctx, _outcome| {
    let o = o2.clone();
    async move {
      o.write().push("second (fails)");
      Err(TestError::Other("first failure".into()))
    }
  })
  .on_finish(move |_ctx, _outcome| {
    let o = o3.clone();
    async move {
      o.write().push("third");
      Err(TestError::Other("second failure".into()))
    }
  });

  let err = p.run(ContextData::new(TestContext::default())).await.unwrap_err();
  // First finish failure wins; the later one is logged only. All three ran, in order.
  assert_eq!(err, TestError::Other("first failure".into()));
  assert_eq!(order.read().as_slice(), &["first", "second (fails)", "third"]);
}

#[tokio::test]
async fn finish_ring_traces_finalizers_and_final_outcome() {
  setup_tracing();
  let mut p = two_step_pipeline();
  p.on_finish(|_ctx, _outcome| async { Ok(()) });
  p.on_finish(|_ctx, _outcome| async { Err(TestError::Other("late failure".into())) });
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  let _ = p.run(ContextData::new(TestContext::default())).await;

  let finalizer_events: Vec<_> = trace
    .events()
    .into_iter()
    .filter(|e| matches!(e.kind, TraceEventKind::FinalizerFinished { .. }))
    .collect();
  assert_eq!(finalizer_events.len(), 2);
  // RunFinished reflects the FINAL outcome: the Ok run was flipped to an error by the
  // failing finish handler.
  assert_run_outcome(
    &trace,
    RunOutcome::Errored {
      step: "on_finish".to_string(),
      message: "late failure".to_string(),
    },
  );
}

#[tokio::test]
async fn partial_runners_and_resolve_plan_never_fire_finish_handlers() {
  setup_tracing();
  // Retto's exact shape: a consuming take-and-restore finish handler. A partial run must
  // not consume the state the test wants to inspect.
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["drain", "install"]);
  p.on_root("drain", |ctx| async move {
    ctx.write().data_for_scoped = Some("drain-id-42".into());
    Ok(PipelineControl::Continue)
  })
  .on_root("install", |_ctx| async { Ok(PipelineControl::Continue) });

  let restores = ExecutionCounter::new();
  let r = restores.clone();
  p.on_finish(move |ctx, _outcome| {
    let r = r.clone();
    async move {
      if ctx.write().data_for_scoped.take().is_some() {
        r.increment();
      }
      Ok(())
    }
  });

  // run_step: post-step state stays inspectable, no finish handler ran.
  let ctx = ContextData::new(TestContext::default());
  p.run_step("drain", ctx.clone()).await.unwrap();
  assert_eq!(ctx.read().data_for_scoped.as_deref(), Some("drain-id-42"));
  assert_eq!(restores.get(), 0);

  // run_from / run_until: same rule.
  let ctx2 = ContextData::new(TestContext::default());
  p.run_from("drain", ctx2.clone()).await.unwrap();
  assert_eq!(ctx2.read().data_for_scoped.as_deref(), Some("drain-id-42"));
  let ctx3 = ContextData::new(TestContext::default());
  p.run_until("drain", ctx3.clone()).await.unwrap();
  assert_eq!(ctx3.read().data_for_scoped.as_deref(), Some("drain-id-42"));
  assert_eq!(restores.get(), 0);

  // resolve_plan executes nothing at all.
  let _plan = p.resolve_plan(&ContextData::new(TestContext::default()));
  assert_eq!(restores.get(), 0);

  // A full run() DOES fire the ring and consumes the drain id.
  let ctx4 = ContextData::new(TestContext::default());
  p.run(ctx4.clone()).await.unwrap();
  assert_eq!(ctx4.read().data_for_scoped, None);
  assert_eq!(restores.get(), 1);
}
