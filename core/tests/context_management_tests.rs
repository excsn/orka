// tests/context_management_tests.rs
mod common;

use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_context_data_is_shared_and_modified() {
  setup_tracing();
  let mut pipeline =
    Pipeline::<TestContext, TestError>::new(&[("step1_modify", false, None), ("step2_read_modify", false, None)]);

  pipeline.on_root("step1_modify", |ctx: ContextData<TestContext>| {
    Box::pin(async move {
      let mut guard = ctx.write();
      guard.counter = 10;
      guard.message = "SetByStep1".to_string();
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });

  pipeline.on_root("step2_read_modify", |ctx: ContextData<TestContext>| {
    Box::pin(async move {
      let mut guard = ctx.write();
      assert_eq!(guard.counter, 10); // Verify value from step1
      assert_eq!(guard.message, "SetByStep1");
      guard.counter += 5;
      guard.message.push_str("_ThenStep2");
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });

  let initial_ctx = ContextData::new(TestContext::default());
  pipeline.run(initial_ctx.clone()).await.unwrap();

  let final_guard = initial_ctx.read();
  assert_eq!(final_guard.counter, 15);
  assert_eq!(final_guard.message, "SetByStep1_ThenStep2");
}

#[tokio::test]
#[serial]
async fn test_context_data_clone_shares_data() {
  setup_tracing();
  let original_ctx = ContextData::new(TestContext {
    counter: 1,
    ..Default::default()
  });
  let cloned_ctx = original_ctx.clone();

  {
    original_ctx.write().counter = 5;
  }
  assert_eq!(cloned_ctx.read().counter, 5); // Clone sees modification

  {
    cloned_ctx.write().counter = 10;
  }
  assert_eq!(original_ctx.read().counter, 10); // Original sees modification
}

// Test for ensuring lock guard is dropped before await (hard to test directly, relies on developer discipline)
// This test can only demonstrate that the code compiles and runs if locks are handled correctly.
#[tokio::test]
#[serial]
async fn test_context_data_locks_with_await() {
  setup_tracing();
  let ctx = ContextData::new(TestContext::default());

  // Simulates a handler
  let handler_logic = async {
    let initial_count = {
      // Scope for read lock
      let guard = ctx.read();
      guard.counter
    }; // Read lock dropped

    tokio::time::sleep(std::time::Duration::from_millis(1)).await; // .await here

    {
      // Scope for write lock
      let mut guard = ctx.write();
      guard.counter = initial_count + 1;
    } // Write lock dropped
  };

  handler_logic.await;
  assert_eq!(ctx.read().counter, 1);
}
