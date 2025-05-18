// tests/error_handling_tests.rs
mod common;
use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_pipeline_run_catches_handler_missing() {
  setup_tracing();
  let pipeline = Pipeline::<TestContext, TestError>::new(&[("missing", false, None)]);
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

// Test specific OrkaError variants from conditional logic (extractor/provider failures)
// are already covered in conditional_scope_tests.rs because TestError::Orka wraps the
// formatted OrkaError string.

// Test a pipeline whose error type IS OrkaError.
#[tokio::test]
#[serial]
async fn test_pipeline_with_orka_error_type() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, OrkaError>::new(&[("task", false, None)]);

  pipeline.on_root("task", |ctx: ContextData<TestContext>| {
    Box::pin(async move {
      ctx.write().counter = 1;
      // If this handler needed to return a specific OrkaError:
      // return Err(OrkaError::Internal("test orka error".to_string()));
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok());
  assert_eq!(ctx.read().counter, 1);

  // Test failing with an OrkaError
  let mut failing_pipeline = Pipeline::<TestContext, OrkaError>::new(&[("fail_task", false, None)]);
  failing_pipeline.on_root("fail_task", |_ctx| {
    Box::pin(async move { Err(OrkaError::Internal("Intentional OrkaError".to_string())) })
  });
  let fail_ctx = ContextData::new(TestContext::default());
  let fail_result = failing_pipeline.run(fail_ctx).await;
  assert!(fail_result.is_err());
  match fail_result.err().unwrap() {
    OrkaError::Internal(s) => assert_eq!(s, "Intentional OrkaError"),
    _ => panic!("Expected OrkaError::Internal"),
  }
}
