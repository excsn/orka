mod common;
use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl};

#[tokio::test]
async fn test_pipeline_run_catches_handler_missing() {
  setup_tracing();
  let pipeline = Pipeline::<TestContext, TestError>::new(["missing"]);
  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx).await;
  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Orka(s) => {
      assert!(s.contains("HandlerMissing"));
      assert!(s.contains("missing"));
    }
    other => panic!("Expected TestError::Orka(HandlerMissing), got {:?}", other),
  }
}

// Extractor and provider failure variants are covered in conditional_scope_tests.rs.

/// A pipeline whose handler error type is `OrkaError` itself, the zero-friction path.
#[tokio::test]
async fn test_pipeline_with_orka_error_type() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, OrkaError>::new(["task"]);

  pipeline.on_root("task", |ctx| async move {
    ctx.write().counter = 1;
    Ok(PipelineControl::Continue)
  });

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok());
  assert_eq!(ctx.read().counter, 1);

  let mut failing_pipeline = Pipeline::<TestContext, OrkaError>::new(["fail_task"]);
  failing_pipeline.on_root("fail_task", |_ctx| async move {
    Err(OrkaError::Internal("Intentional OrkaError".to_string()))
  });
  let fail_ctx = ContextData::new(TestContext::default());
  let fail_result = failing_pipeline.run(fail_ctx).await;
  assert!(fail_result.is_err());
  match fail_result.err().unwrap() {
    OrkaError::Internal(s) => assert_eq!(s, "Intentional OrkaError"),
    _ => panic!("Expected OrkaError::Internal"),
  }
}

/// A handler may return any error convertible into the pipeline's `Err` via `?`.
#[tokio::test]
async fn test_handler_converts_foreign_error_with_question_mark() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["parse"]);

  pipeline.on_root("parse", |ctx| async move {
    let parsed: i32 = "not_a_number"
      .parse()
      .map_err(|e| TestError::Handler(format!("parse failed: {}", e)))?;
    ctx.write().counter = parsed;
    Ok(PipelineControl::Continue)
  });

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  assert_eq!(ctx.read().counter, 0);
}

/// Orka owns no timer, so a timeout is the handler's own business. What orka provides is
/// the reporting: `StepTimedOut` carries the step's name into `run_with_outcome` and the
/// trace, which is what a hand-rolled timeout loses unless every call site remembers to
/// encode it.
#[tokio::test]
async fn a_timed_out_step_names_itself_in_the_run_outcome() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["quick", "slow"]);

  pipeline.on_root("quick", |ctx| async move {
    ctx.write().counter += 1;
    Ok(PipelineControl::Continue)
  });
  pipeline.on_root("slow", |_ctx| async move {
    let budget = std::time::Duration::from_millis(10);
    match tokio::time::timeout(budget, tokio::time::sleep(std::time::Duration::from_secs(30))).await {
      Ok(()) => Ok(PipelineControl::Continue),
      Err(_) => Err(TestError::from(OrkaError::StepTimedOut {
        step_name: "slow".to_string(),
        after: budget,
      })),
    }
  });

  let ctx = ContextData::new(TestContext::default());
  let (result, outcome) = pipeline.run_with_outcome(ctx.clone()).await;

  assert!(result.is_err());
  assert_eq!(ctx.read().counter, 1, "the earlier step still ran");

  match outcome {
    orka::RunOutcome::Errored { step, message } => {
      assert_eq!(step, "slow", "the failure is attributed to the step that timed out");
      assert!(message.contains("StepTimedOut"), "message was: {}", message);
    }
    other => panic!("expected Errored, got {:?}", other),
  }
}

/// `timed` collapses the match-and-map that every hand-rolled timeout repeats, and keeps
/// the step name on the error so the failure is attributable on the plain `run()` path.
#[tokio::test]
async fn timed_reports_an_overrun_as_a_step_timeout() {
  setup_tracing();
  let budget = std::time::Duration::from_millis(10);

  let mut pipeline = Pipeline::<TestContext, TestError>::new(["await_artifact"]);
  pipeline.on_root("await_artifact", move |_ctx| async move {
    // Stands in for a remote push that never arrives.
    orka::timed("await_artifact", budget, tokio::time::sleep(std::time::Duration::from_secs(30))).await?;
    Ok(PipelineControl::Continue)
  });

  let (result, outcome) = pipeline
    .run_with_outcome(ContextData::new(TestContext::default()))
    .await;

  match result.unwrap_err() {
    TestError::Orka(s) => {
      assert!(s.contains("StepTimedOut"), "got: {}", s);
      assert!(s.contains("await_artifact"), "the error names the step: {}", s);
    }
    other => panic!("expected the framework error, got {:?}", other),
  }
  assert!(matches!(outcome, orka::RunOutcome::Errored { ref step, .. } if step == "await_artifact"));
}

/// A future that finishes inside its budget passes its value straight through, so the
/// helper is a drop-in around an existing await.
#[tokio::test]
async fn timed_passes_the_value_through_when_it_finishes_in_time() {
  setup_tracing();
  let budget = std::time::Duration::from_secs(30);

  let doubled = orka::timed("quick", budget, async { 21 * 2 }).await.unwrap();
  assert_eq!(doubled, 42);

  // The inner future's own Result is untouched, which is why the handler case needs two
  // unwraps: one for the timeout, one for the operation.
  let inner: Result<i32, &str> = orka::timed("quick", budget, async { Ok(7) }).await.unwrap();
  assert_eq!(inner.unwrap(), 7);
}
