mod common;

use common::*;
use orka::{ContextData, Pipeline, PipelineControl};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_context_data_is_shared_and_modified() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step1_modify", "step2_read_modify"]);

  pipeline
    .on_root("step1_modify", |ctx| async move {
      let mut guard = ctx.write();
      guard.counter = 10;
      guard.message = "SetByStep1".to_string();
      Ok(PipelineControl::Continue)
    })
    .on_root("step2_read_modify", |ctx| async move {
      let mut guard = ctx.write();
      assert_eq!(guard.counter, 10, "step2 must observe step1's write");
      assert_eq!(guard.message, "SetByStep1");
      guard.counter += 5;
      guard.message.push_str("_ThenStep2");
      Ok(PipelineControl::Continue)
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
  assert_eq!(cloned_ctx.read().counter, 5, "clone observes the original's write");

  {
    cloned_ctx.write().counter = 10;
  }
  assert_eq!(original_ctx.read().counter, 10, "original observes the clone's write");
}

/// `project` builds an independent `ContextData` from part of another, so writes to the
/// projection do not touch the source.
#[tokio::test]
#[serial]
async fn test_context_data_project_is_independent() {
  setup_tracing();
  let ctx = ContextData::new(MainExtractContext {
    sub_data_container: SubExtractContext {
      sub_field: "original".to_string(),
      processed: false,
    },
    ..Default::default()
  });

  let projected = ctx.project(|d| d.sub_data_container.clone());
  projected.write().processed = true;

  assert!(projected.read().processed);
  assert!(
    !ctx.read().sub_data_container.processed,
    "a projection is detached from its source"
  );
}

/// Guards are blocking and must not be held across `.await`. This documents the required
/// scoping discipline; it compiles and runs only if the guards are dropped as shown.
#[tokio::test]
#[serial]
async fn test_context_data_locks_with_await() {
  setup_tracing();
  let ctx = ContextData::new(TestContext::default());

  let handler_logic = async {
    let initial_count = {
      let guard = ctx.read();
      guard.counter
    };

    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    {
      let mut guard = ctx.write();
      guard.counter = initial_count + 1;
    }
  };

  handler_logic.await;
  assert_eq!(ctx.read().counter, 1);
}
