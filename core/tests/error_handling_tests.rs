mod common;
use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use serial_test::serial;

#[tokio::test]
#[serial]
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

/// A pipeline whose handler error type is `OrkaError` itself — the zero-friction path.
#[tokio::test]
#[serial]
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
#[serial]
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
