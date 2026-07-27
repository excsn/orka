mod common;

use common::*;
use orka::{ContextData, Pipeline, PipelineControl};

#[tokio::test]
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

/// `with_ref`/`with_mut` make the "no guard across `.await`" rule structural rather than a
/// convention: the closure is synchronous and the guard's scope is the call, so the lock
/// cannot reach a suspension point. Compare the explicit scoping blocks above.
#[tokio::test]
async fn test_with_ref_and_with_mut_are_await_safe() {
  setup_tracing();
  let ctx = ContextData::new(TestContext::default());

  let initial = ctx.with_ref(|c| c.counter);
  tokio::time::sleep(std::time::Duration::from_millis(1)).await;
  ctx.with_mut(|c| c.counter = initial + 1);

  // Back-to-back calls: each guard must have been released before returning, or this
  // would deadlock rather than fail.
  ctx.with_mut(|c| c.counter += 1);
  assert_eq!(ctx.with_ref(|c| c.counter), 2);
}

/// A non-`Clone` intermediate threaded through steps needs no `Arc` and no
/// `try_unwrap` -> mutate -> re-wrap dance: `with_mut` hands out a scoped `&mut` to the
/// field, and returns a value, so taking ownership back out is one line.
#[tokio::test]
async fn test_with_mut_handles_non_clone_intermediates_in_place() {
  setup_tracing();

  // Deliberately neither Clone nor Default: the shape of parsed specs or a compiled
  // artifact that one step produces and a later step mutates.
  #[derive(Debug, PartialEq)]
  struct ParsedSpecs {
    entries: Vec<String>,
  }

  #[derive(Default)]
  struct BuildCtx {
    specs: Option<ParsedSpecs>,
  }

  let ctx = ContextData::new(BuildCtx::default());

  // "parse" step produces it.
  ctx.with_mut(|c| {
    c.specs = Some(ParsedSpecs {
      entries: vec!["alpha".to_string()],
    })
  });

  // "mutate" step edits it in place: no Arc, no clone, no unwrap dance.
  ctx.with_mut(|c| c.specs.as_mut().expect("set by parse step").entries.push("beta".to_string()));

  // And ownership can be taken back out through the return value.
  let taken = ctx.with_mut(|c| c.specs.take()).expect("still present");
  assert_eq!(
    taken,
    ParsedSpecs {
      entries: vec!["alpha".to_string(), "beta".to_string()]
    }
  );
  assert!(ctx.with_ref(|c| c.specs.is_none()));
}
