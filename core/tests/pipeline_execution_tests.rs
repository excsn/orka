// tests/pipeline_execution_tests.rs
mod common; // Reference the common module

use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use serial_test::serial;
use std::sync::Arc;

#[tokio::test]
#[serial]
async fn test_pipeline_runs_steps_in_order() {
  setup_tracing();
  let mut pipeline =
    Pipeline::<TestContext, TestError>::new(&[("step1", false, None), ("step2", false, None), ("step3", false, None)]);

  pipeline.on_root("step1", create_simple_handler("step1", " S1"));
  pipeline.on_root("step2", create_simple_handler("step2", " S2"));
  pipeline.on_root("step3", create_simple_handler("step3", " S3"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.counter, 3);
  assert_eq!(guard.message, " S1 S2 S3");
  assert_eq!(guard.steps_executed, vec!["step1", "step2", "step3"]);
}

#[tokio::test]
#[serial]
async fn test_pipeline_stops_on_pipeline_control_stop() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("stepA", false, None),
    ("stopStep", false, None),
    ("stepC", false, None),
  ]);

  pipeline.on_root("stepA", create_simple_handler("stepA", "A"));
  pipeline.on_root("stopStep", |ctx: ContextData<TestContext>| {
    Box::pin(async move {
      ctx.write().steps_executed.push("stopStep".to_string());
      Ok::<PipelineControl, OrkaError>(PipelineControl::Stop)
    })
  });
  pipeline.on_root("stepC", create_simple_handler("stepC", "C")); // This should not run

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Stopped);

  let guard = ctx.read();
  assert_eq!(guard.counter, 1); // Only stepA incremented
  assert_eq!(guard.message, "A");
  assert_eq!(guard.steps_executed, vec!["stepA", "stopStep"]);
}

#[tokio::test]
#[serial]
async fn test_pipeline_propagates_handler_error() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("good_step", false, None),
    ("bad_step", false, None),
    ("another_step", false, None),
  ]);

  pipeline.on_root("good_step", create_simple_handler("good_step", "Good"));
  pipeline.on_root("bad_step", create_failing_handler("bad_step", "I am a bad step!"));
  pipeline.on_root("another_step", create_simple_handler("another_step", "NeverRun"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Handler(msg) => assert_eq!(msg, "I am a bad step!"),
    _ => panic!("Expected TestError::Handler"),
  }

  let guard = ctx.read();
  assert_eq!(guard.counter, 1); // Only good_step incremented
  assert_eq!(guard.message, "Good");
  assert_eq!(guard.steps_executed, vec!["good_step", "bad_step"]);
}

#[tokio::test]
#[serial]
async fn test_pipeline_skips_step_if_condition_met() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("step1", false, None),
    (
      "step_to_skip",
      false,
      Some(Arc::new(|ctx: ContextData<TestContext>| ctx.read().counter > 0)),
    ),
    ("step3", false, None),
  ]);

  pipeline.on_root("step1", create_simple_handler("step1", " S1"));
  pipeline.on_root("step_to_skip", create_simple_handler("step_to_skip", " SKIPPED_THIS"));
  pipeline.on_root("step3", create_simple_handler("step3", " S3"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert_eq!(result.unwrap(), PipelineResult::Completed);
  let guard = ctx.read();
  assert_eq!(guard.counter, 2); // step1 and step3 ran
  assert_eq!(guard.message, " S1 S3");
  assert_eq!(guard.steps_executed, vec!["step1", "step3"]);
}

#[tokio::test]
#[serial]
async fn test_non_optional_step_missing_handler_fails() {
  setup_tracing();
  let pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("step_with_no_handler", false, None), // Non-optional
  ]);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  if let Err(TestError::Orka(s)) = result {
    assert!(s.contains("HandlerMissing"));
    assert!(s.contains("step_with_no_handler"));
  } else {
    panic!("Expected OrkaError::HandlerMissing, got {:?}", result);
  }
}

#[tokio::test]
#[serial]
async fn test_optional_step_missing_handler_succeeds() {
  setup_tracing();
  let pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("optional_step_no_handler", true, None), // Optional
  ]);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Completed);
}

#[tokio::test]
#[serial]
async fn test_before_on_after_execution_order() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("main_step", false, None)]);

  pipeline.before_root("main_step", create_simple_handler("before_main", "Before;"));
  pipeline.on_root("main_step", create_simple_handler("on_main", "On;"));
  pipeline.after_root("main_step", create_simple_handler("after_main", "After;"));

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx.clone()).await.unwrap();

  let guard = ctx.read();
  assert_eq!(guard.counter, 3);
  assert_eq!(guard.message, "Before;On;After;");
  assert_eq!(guard.steps_executed, vec!["before_main", "on_main", "after_main"]);
}

#[tokio::test]
async fn test_sub_context_extraction_and_handler_success() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(&[("extract_step", false, None)]);

  // 1. Set Extractor: Extracts SubExtractContext from MainExtractContext
  pipeline.set_extractor("extract_step", |main_ctx: ContextData<MainExtractContext>| {
    // This extractor creates a *new* ContextData<SubExtractContext> that shares
    // the sub_data_container's RwLock from the main context.
    // This requires careful mapping of locks if direct mutation of main_ctx's field is desired.
    // A simpler approach for fully independent SData is to clone the data.
    // For this test, let's make SData a *view* or *part* of TData.
    //
    // To allow the sub-handler to modify a part of the main context,
    // the extractor should return a ContextData that, when written to,
    // modifies the original MainExtractContext. This is non-trivial if SubExtractContext
    // is not directly Arc<RwLock>ed within MainExtractContext.
    //
    // Alternative: Extractor provides a CLONE of the sub-data.
    // The sub-handler modifies its own copy.
    // An after_root handler on main_ctx would then merge changes if needed.
    //
    // For this test, let's assume the sub-handler modifies its data, and we check that.
    // If SData is a struct field of TData, the extractor must return a ContextData
    // that wraps an Arc<RwLock<SData>> which is effectively a *part of* TData.
    // The current ContextDataExtractorImpl is generic.
    // Let's modify MainExtractContext slightly for a more direct test of modification.

    // For this test to show modification back on main_ctx, SData needs to be a ref or share lock.
    // The current `ContextDataExtractorImpl` creates a *new* `ContextData<SData>`.
    // To test shared modification, we'd need a more complex extractor or context structure.
    // Let's test that the sub-handler runs and can modify *its own* `SData`.

    let sub_data_clone = main_ctx.read().sub_data_container.clone();
    Ok(ContextData::new(sub_data_clone))
  });

  // 2. Register Handler for SubExtractContext
  pipeline.on::<SubExtractContext, _, TestError>("extract_step", |sctx: ContextData<SubExtractContext>| {
    Box::pin(async move {
      let mut s_guard = sctx.write();
      s_guard.sub_field = "ProcessedBySubHandler".to_string();
      s_guard.processed = true;
      // How to signal back to main context for assertion?
      // One way: sub-handler sets a flag. Main context's after_root handler reads it.
      // For this simple test, we will inspect the main context *after* to see if it was
      // impacted, assuming the extractor was set up to share state (which is hard with current ContextDataExtractorImpl).
      //
      // Simpler test: just confirm sub-handler ran. We'll need a different way to assert.
      // Let's use a counter on the main context, incremented by a root handler *after* the sub-handler.
      Ok(PipelineControl::Continue)
    })
  });

  // Add a root handler to check/integrate changes if SData was a clone
  pipeline.after_root("extract_step", |main_ctx: ContextData<MainExtractContext>| {
    Box::pin(async move {
      // This part is tricky if SData was a totally independent clone.
      // If the extractor set up shared state, we could see changes here.
      // For now, let's just increment a counter to show this part ran.
      main_ctx.write().counter += 1;
      main_ctx.write().steps_executed.push("after_extract_step".to_string());
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  let initial_main_ctx = MainExtractContext {
    main_field: "main".to_string(),
    sub_data_container: SubExtractContext {
      sub_field: "initial_sub".to_string(),
      processed: false,
    },
    counter: 0,
    steps_executed: vec![],
  };
  let ctx = ContextData::new(initial_main_ctx);
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.counter, 1); // Verifies after_root ran
  assert_eq!(guard.steps_executed, vec!["after_extract_step"]);
  // To verify sub_data_container.processed, the sub-handler would need to modify
  // the original data. With the current extractor providing a clone, this won't reflect.
  // This test currently verifies the *flow* more than shared state mutation via sub-handler.
  // A better test for shared state would involve MainExtractContext.sub_data_container being ContextData<SubExtractContext>
  // or the extractor returning a MappedRwLockWriteGuard effectively.
  //
  // Given current extractor design:
  // The sub_handler modified its own copy. The original main_ctx.sub_data_container is unchanged.
  assert_eq!(guard.sub_data_container.processed, false); // Because sub-handler worked on a clone
  assert_eq!(guard.sub_data_container.sub_field, "initial_sub");
  // To really test the sub-handler's effect, we need a side channel or a different context setup.
  // For now, the after_root handler running proves the on<SData> completed with Continue.
}

#[tokio::test]
async fn test_sub_context_extractor_fails() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(&[("extract_fail_step", false, None)]);

  pipeline.set_extractor("extract_fail_step", |_main_ctx: ContextData<MainExtractContext>| {
    Err::<ContextData<MainExtractContext>, _>(OrkaError::ExtractorFailure {
      step_name: "test_failing_extractor".to_string(),
      source: anyhow::anyhow!("Intentional extractor failure"),
    })
  });

  pipeline.on::<SubExtractContext, _, TestError>("extract_fail_step", |sctx: ContextData<SubExtractContext>| {
    Box::pin(async move {
      // This handler should not be called
      sctx.write().processed = true;
      panic!("Sub-handler should not have been called after extractor failure!");
      // Ok(PipelineControl::Continue)
    })
  });

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Orka(s) => {
      assert!(s.contains("ExtractorFailure"));
      assert!(s.contains("Intentional extractor failure"));
    }
    other => panic!("Expected TestError::Orka(ExtractorFailure), got {:?}", other),
  }
  assert_eq!(ctx.read().sub_data_container.processed, false); // Ensure sub-handler didn't run
}

#[tokio::test]
async fn test_sub_context_type_mismatch() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(&[("type_mismatch_step", false, None)]);

  // Extractor provides SubExtractContext
  pipeline.set_extractor("type_mismatch_step", |main_ctx: ContextData<MainExtractContext>| {
    let sub_data_clone = main_ctx.read().sub_data_container.clone();
    Ok(ContextData::new(sub_data_clone)) // Correctly provides ContextData<SubExtractContext>
  });

  // But handler expects OtherSubContext
  pipeline.on::<OtherSubContext, _, TestError>("type_mismatch_step", |_sctx: ContextData<OtherSubContext>| {
    Box::pin(async move {
      // This handler should not be called successfully
      panic!("Sub-handler with mismatched type should not execute successfully!");
      // Ok(PipelineControl::Continue)
    })
  });

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Orka(s) => {
      assert!(s.contains("TypeMismatch") || s.contains("Internal type mismatch during ContextData downcast"));
      // OrkaError::TypeMismatch
    }
    other => panic!("Expected TestError::Orka(TypeMismatch), got {:?}", other),
  }
}

#[tokio::test]
async fn test_sub_context_handler_fails() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(&[("sub_handler_fail_step", false, None)]);

  pipeline.set_extractor("sub_handler_fail_step", |main_ctx: ContextData<MainExtractContext>| {
    let sub_data_clone = main_ctx.read().sub_data_container.clone();
    Ok(ContextData::new(sub_data_clone))
  });

  pipeline.on::<SubExtractContext, _, TestError>("sub_handler_fail_step", |_sctx: ContextData<SubExtractContext>| {
    Box::pin(async move { Err(TestError::Handler("Sub-handler failed intentionally".to_string())) })
  });
  // Add a root handler to check if it runs
  pipeline.after_root::<_, OrkaError>("sub_handler_fail_step", |main_ctx: ContextData<MainExtractContext>| {
    Box::pin(async move {
      main_ctx
        .write()
        .steps_executed
        .push("after_sub_handler_fail".to_string());
      panic!("after_root should not run if on<SData> handler failed");
      // Ok(PipelineControl::Continue)
    })
  });

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  assert_eq!(
    result.err().unwrap(),
    TestError::Handler("Sub-handler failed intentionally".to_string())
  );
  assert!(ctx.read().steps_executed.is_empty()); // after_root should not have run
}

#[tokio::test]
async fn test_sub_context_handler_stops_pipeline() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(&[
    ("sub_handler_stop_step", false, None),
    ("after_stop_step", false, None), // This root step should not run
  ]);

  pipeline.set_extractor("sub_handler_stop_step", |main_ctx: ContextData<MainExtractContext>| {
    let sub_data_clone = main_ctx.read().sub_data_container.clone();
    Ok(ContextData::new(sub_data_clone))
  });

  pipeline.on::<SubExtractContext, _, TestError>("sub_handler_stop_step", |_sctx: ContextData<SubExtractContext>| {
    Box::pin(async move { Ok(PipelineControl::Stop) })
  });
  pipeline.on_root(
    "after_stop_step",
    create_main_extract_context_simple_handler("after_stop_step", "ShouldNotRun"),
  );

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Stopped);
  assert!(ctx.read().steps_executed.is_empty()); // after_stop_step handler should not have run
}
