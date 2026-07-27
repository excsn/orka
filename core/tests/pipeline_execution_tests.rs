mod common;

use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};

#[tokio::test]
async fn test_pipeline_runs_steps_in_order() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step1", "step2", "step3"]);

  pipeline
    .on_root("step1", create_simple_handler("step1", " S1"))
    .on_root("step2", create_simple_handler("step2", " S2"))
    .on_root("step3", create_simple_handler("step3", " S3"));

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
async fn test_pipeline_stops_on_pipeline_control_stop() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["stepA", "stopStep", "stepC"]);

  pipeline
    .on_root("stepA", create_simple_handler("stepA", "A"))
    .on_root("stopStep", |ctx| async move {
      ctx.write().steps_executed.push("stopStep".to_string());
      Ok(PipelineControl::Stop)
    })
    .on_root("stepC", create_simple_handler("stepC", "C"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Stopped);

  let guard = ctx.read();
  assert_eq!(guard.counter, 1);
  assert_eq!(guard.message, "A");
  assert_eq!(guard.steps_executed, vec!["stepA", "stopStep"]);
}

#[tokio::test]
async fn test_pipeline_propagates_handler_error() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["good_step", "bad_step", "another_step"]);

  pipeline
    .on_root("good_step", create_simple_handler("good_step", "Good"))
    .on_root("bad_step", create_failing_handler("bad_step", "I am a bad step!"))
    .on_root("another_step", create_simple_handler("another_step", "NeverRun"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Handler(msg) => assert_eq!(msg, "I am a bad step!"),
    _ => panic!("Expected TestError::Handler"),
  }

  let guard = ctx.read();
  assert_eq!(guard.counter, 1);
  assert_eq!(guard.message, "Good");
  assert_eq!(guard.steps_executed, vec!["good_step", "bad_step"]);
}

#[tokio::test]
async fn test_pipeline_skips_step_if_condition_met() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step1", "step_to_skip", "step3"]);

  pipeline
    .skip_if("step_to_skip", |ctx: ContextData<TestContext>| ctx.read().counter > 0)
    .on_root("step1", create_simple_handler("step1", " S1"))
    .on_root("step_to_skip", create_simple_handler("step_to_skip", " SKIPPED_THIS"))
    .on_root("step3", create_simple_handler("step3", " S3"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert_eq!(result.unwrap(), PipelineResult::Completed);
  let guard = ctx.read();
  assert_eq!(guard.counter, 2);
  assert_eq!(guard.message, " S1 S3");
  assert_eq!(guard.steps_executed, vec!["step1", "step3"]);
}

#[tokio::test]
async fn test_clear_skip_condition_reinstates_step() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step1", "step2"]);

  pipeline
    .skip_if("step2", |_| true)
    .clear_skip_condition("step2")
    .on_root("step1", create_simple_handler("step1", " S1"))
    .on_root("step2", create_simple_handler("step2", " S2"));

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx.clone()).await.unwrap();

  assert_eq!(ctx.read().steps_executed, vec!["step1", "step2"]);
}

#[tokio::test]
async fn test_non_optional_step_missing_handler_fails() {
  setup_tracing();
  let pipeline = Pipeline::<TestContext, TestError>::new(["step_with_no_handler"]);

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
async fn test_optional_step_missing_handler_succeeds() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["optional_step_no_handler"]);
  pipeline.optional("optional_step_no_handler");

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Completed);
}

#[tokio::test]
async fn test_required_reverts_optional() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step"]);
  pipeline.optional("step").required("step");

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err(), "a required step with no handlers must fail");
}

#[tokio::test]
async fn test_before_on_after_execution_order() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["main_step"]);

  pipeline
    .before_root("main_step", create_simple_handler("before_main", "Before;"))
    .on_root("main_step", create_simple_handler("on_main", "On;"))
    .after_root("main_step", create_simple_handler("after_main", "After;"));

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx.clone()).await.unwrap();

  let guard = ctx.read();
  assert_eq!(guard.counter, 3);
  assert_eq!(guard.message, "Before;On;After;");
  assert_eq!(guard.steps_executed, vec!["before_main", "on_main", "after_main"]);
}

#[tokio::test]
async fn test_sub_context_extraction_is_detached_by_default() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["extract_step"]);

  // `set_extractor` hands the sub-handler an independent copy, so its writes stay local.
  pipeline
    .set_extractor("extract_step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on("extract_step", |sctx: ContextData<SubExtractContext>| async move {
      let mut s_guard = sctx.write();
      s_guard.sub_field = "ProcessedBySubHandler".to_string();
      s_guard.processed = true;
      Ok(PipelineControl::Continue)
    })
    .after_root("extract_step", |main_ctx: ContextData<MainExtractContext>| async move {
      let mut guard = main_ctx.write();
      guard.counter += 1;
      guard.steps_executed.push("after_extract_step".to_string());
      Ok(PipelineControl::Continue)
    });

  let ctx = ContextData::new(MainExtractContext {
    main_field: "main".to_string(),
    sub_data_container: SubExtractContext {
      sub_field: "initial_sub".to_string(),
      processed: false,
    },
    counter: 0,
    steps_executed: vec![],
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.counter, 1, "after_root ran, so on<SData> returned Continue");
  assert_eq!(guard.steps_executed, vec!["after_extract_step"]);
  assert!(!guard.sub_data_container.processed);
  assert_eq!(guard.sub_data_container.sub_field, "initial_sub");
}

#[tokio::test]
async fn test_sub_context_with_merge_writes_back_to_root() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["extract_step"]);

  pipeline
    .set_extractor_with_merge(
      "extract_step",
      |main_ctx: ContextData<MainExtractContext>| Ok(main_ctx.project(|d| d.sub_data_container.clone())),
      |root, sub| root.sub_data_container = sub.clone(),
    )
    .on("extract_step", |sctx: ContextData<SubExtractContext>| async move {
      let mut s_guard = sctx.write();
      s_guard.sub_field = "ProcessedBySubHandler".to_string();
      s_guard.processed = true;
      Ok(PipelineControl::Continue)
    });

  let ctx = ContextData::new(MainExtractContext {
    sub_data_container: SubExtractContext {
      sub_field: "initial_sub".to_string(),
      processed: false,
    },
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline failed: {:?}", result.err());

  let guard = ctx.read();
  assert!(guard.sub_data_container.processed);
  assert_eq!(guard.sub_data_container.sub_field, "ProcessedBySubHandler");
}

#[tokio::test]
async fn test_sub_context_merge_is_skipped_when_handler_fails() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["extract_step"]);

  pipeline
    .set_extractor_with_merge(
      "extract_step",
      |main_ctx: ContextData<MainExtractContext>| Ok(main_ctx.project(|d| d.sub_data_container.clone())),
      |root, sub| root.sub_data_container = sub.clone(),
    )
    .on("extract_step", |sctx: ContextData<SubExtractContext>| async move {
      sctx.write().processed = true;
      Err(TestError::Handler("sub-handler failed".to_string()))
    });

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  assert!(
    !ctx.read().sub_data_container.processed,
    "a failed sub-handler must not be merged back"
  );
}

#[tokio::test]
async fn test_sub_context_extractor_fails() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["extract_fail_step"]);

  pipeline
    .set_extractor("extract_fail_step", |_main_ctx: ContextData<MainExtractContext>| {
      Err::<ContextData<MainExtractContext>, _>(OrkaError::ExtractorFailure {
        step_name: "test_failing_extractor".to_string(),
        source: anyhow::anyhow!("Intentional extractor failure"),
      })
    })
    .on("extract_fail_step", |sctx: ContextData<SubExtractContext>| async move {
      sctx.write().processed = true;
      panic!("Sub-handler should not have been called after extractor failure!");
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
  assert!(!ctx.read().sub_data_container.processed);
}

#[tokio::test]
async fn test_sub_context_type_mismatch() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["type_mismatch_step"]);

  // The extractor yields SubExtractContext but the handler asks for OtherSubContext.
  pipeline
    .set_extractor("type_mismatch_step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on("type_mismatch_step", |_sctx: ContextData<OtherSubContext>| async move {
      panic!("Sub-handler with mismatched type should not execute successfully!");
    });

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  match result.err().unwrap() {
    TestError::Orka(s) => {
      assert!(s.contains("TypeMismatch") || s.contains("Internal type mismatch during ContextData downcast"));
    }
    other => panic!("Expected TestError::Orka(TypeMismatch), got {:?}", other),
  }
}

#[tokio::test]
async fn test_sub_context_handler_fails() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["sub_handler_fail_step"]);

  pipeline
    .set_extractor("sub_handler_fail_step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on("sub_handler_fail_step", |_sctx: ContextData<SubExtractContext>| async move {
      Err(TestError::Handler("Sub-handler failed intentionally".to_string()))
    })
    .after_root(
      "sub_handler_fail_step",
      |main_ctx: ContextData<MainExtractContext>| async move {
        main_ctx
          .write()
          .steps_executed
          .push("after_sub_handler_fail".to_string());
        panic!("after_root should not run if on<SData> handler failed");
      },
    );

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  assert_eq!(
    result.err().unwrap(),
    TestError::Handler("Sub-handler failed intentionally".to_string())
  );
  assert!(ctx.read().steps_executed.is_empty());
}

#[tokio::test]
async fn test_sub_context_handler_stops_pipeline() {
  setup_tracing();
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["sub_handler_stop_step", "after_stop_step"]);

  pipeline
    .set_extractor("sub_handler_stop_step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on(
      "sub_handler_stop_step",
      |_sctx: ContextData<SubExtractContext>| async move { Ok(PipelineControl::Stop) },
    )
    .on_root(
      "after_stop_step",
      create_main_extract_context_simple_handler("after_stop_step", "ShouldNotRun"),
    );

  let ctx = ContextData::new(MainExtractContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok());
  assert_eq!(result.unwrap(), PipelineResult::Stopped);
  assert!(ctx.read().steps_executed.is_empty());
}

#[tokio::test]
async fn test_insert_before_and_after_step() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["middle"]);

  pipeline
    .insert_before_step("middle", "first")
    .insert_after_step("middle", "last")
    .on_root("first", create_simple_handler("first", "1"))
    .on_root("middle", create_simple_handler("middle", "2"))
    .on_root("last", create_simple_handler("last", "3"));

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx.clone()).await.unwrap();

  assert_eq!(ctx.read().steps_executed, vec!["first", "middle", "last"]);
}

#[tokio::test]
async fn test_remove_step_drops_its_handlers() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["keep", "drop_me"]);

  pipeline
    .on_root("keep", create_simple_handler("keep", "K"))
    .on_root("drop_me", create_simple_handler("drop_me", "D"))
    .remove_step("drop_me");

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx.clone()).await.unwrap();

  assert_eq!(ctx.read().steps_executed, vec!["keep"]);
}

#[tokio::test]
#[should_panic(expected = "duplicate step")]
async fn test_duplicate_step_names_panic() {
  let _ = Pipeline::<TestContext, TestError>::new(["dup", "dup"]);
}

#[tokio::test]
#[should_panic(expected = "not found")]
async fn test_registering_handler_for_unknown_step_panics() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["known"]);
  pipeline.on_root("typo", create_simple_handler("typo", "T"));
}
