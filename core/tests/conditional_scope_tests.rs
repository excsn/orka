mod common;

use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use serial_test::serial;
use std::sync::{atomic::Ordering, Arc};

// --- Scoped pipeline factories ---
//
// The `_counting_` variants bump global counters, so tests using them must be #[serial].
// Prefer the plain variants plus `.with_merge(..)`, which let a test assert on the merged
// context instead of on shared global state.

fn scoped_pipeline_a_factory(
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextA> {
  move |_main_ctx| {
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(["scoped_a_task"]);
    p.on_root("scoped_a_task", |s_ctx: ContextData<ScopedTestContextA>| async move {
      let mut guard = s_ctx.write();
      guard.processed_message = format!("A processed: {}", guard.input);
      Ok(PipelineControl::Continue)
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn scoped_pipeline_b_factory(
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextB> {
  move |_main_ctx| {
    let mut p = Pipeline::<ScopedTestContextB, TestError>::new(["scoped_b_task"]);
    p.on_root("scoped_b_task", |s_ctx: ContextData<ScopedTestContextB>| async move {
      let mut guard = s_ctx.write();
      guard.alternative_message = format!("B alternative: {}", guard.input);
      Ok(PipelineControl::Continue)
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn counting_scoped_pipeline_a_factory(
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextA> {
  move |_main_ctx| {
    PROVIDER_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(["scoped_a_task"]);
    p.on_root("scoped_a_task", |s_ctx: ContextData<ScopedTestContextA>| async move {
      SCOPED_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
      let mut guard = s_ctx.write();
      guard.processed_message = format!("A processed: {}", guard.input);
      Ok(PipelineControl::Continue)
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn failing_provider_factory(
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextA> {
  move |_main_ctx| {
    std::future::ready(Err(OrkaError::PipelineProviderFailure {
      step_name: "failing_provider".to_string(),
      source: anyhow::anyhow!("Provider intentionally failed"),
    }))
  }
}

fn failing_scoped_pipeline_factory(
  failure_message: &'static str,
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextA> {
  move |_main_ctx| {
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(["failing_scoped_task"]);
    p.on_root("failing_scoped_task", move |_s_ctx| async move {
      Err(TestError::ScopedTask(failure_message.to_string()))
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn counting_failing_scoped_pipeline_factory(
  failure_message: &'static str,
) -> impl Fn(ContextData<TestContext>) -> ReadyScopedPipeline<ScopedTestContextA> {
  move |_main_ctx| {
    PROVIDER_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(["failing_scoped_task"]);
    p.on_root("failing_scoped_task", move |_s_ctx| async move {
      SCOPED_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
      Err(TestError::ScopedTask(failure_message.to_string()))
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

/// Builds a main pipeline with two mutually exclusive scopes, each merging its result back
/// into the main context. Because nothing global is touched, this needs no `#[serial]`.
fn pipeline_with_two_merging_scopes() -> Pipeline<TestContext, TestError> {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["conditional_step"]);

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(scoped_pipeline_a_factory(), |main_ctx: ContextData<TestContext>| {
      Ok(main_ctx.project(|d| ScopedTestContextA {
        input: d.data_for_scoped.clone().unwrap_or_default(),
        ..Default::default()
      }))
    })
    .with_merge(|main, sub| {
      main.scoped_a_ran = true;
      main.scoped_result = Some(sub.processed_message.clone());
    })
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "run_a")
    .add_dynamic_scope(scoped_pipeline_b_factory(), |main_ctx: ContextData<TestContext>| {
      Ok(main_ctx.project(|d| ScopedTestContextB {
        input: d.data_for_scoped.clone().unwrap_or_default(),
        ..Default::default()
      }))
    })
    .with_merge(|main, sub| {
      main.scoped_b_ran = true;
      main.scoped_result = Some(sub.alternative_message.clone());
    })
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "run_b")
    .finalize_conditional_step(false);

  pipeline
}

#[tokio::test]
async fn test_conditional_scope_a_runs_when_condition_met() {
  setup_tracing();
  let pipeline = pipeline_with_two_merging_scopes();

  let ctx = ContextData::new(TestContext {
    message: "run_a".to_string(),
    data_for_scoped: Some("data_a".to_string()),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert!(guard.scoped_a_ran, "scope A should have run");
  assert!(!guard.scoped_b_ran, "scope B should not have run");
  assert_eq!(guard.scoped_result.as_deref(), Some("A processed: data_a"));
}

#[tokio::test]
async fn test_conditional_scope_b_runs_when_condition_met() {
  setup_tracing();
  let pipeline = pipeline_with_two_merging_scopes();

  let ctx = ContextData::new(TestContext {
    message: "run_b".to_string(),
    data_for_scoped: Some("data_b".to_string()),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert!(!guard.scoped_a_ran, "scope A should not have run");
  assert!(guard.scoped_b_ran, "scope B should have run");
  assert_eq!(guard.scoped_result.as_deref(), Some("B alternative: data_b"));
}

#[tokio::test]
async fn test_conditional_no_scope_matches_leaves_context_untouched() {
  setup_tracing();
  let pipeline = pipeline_with_two_merging_scopes();

  let ctx = ContextData::new(TestContext {
    message: "matches_nothing".to_string(),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert_eq!(result.unwrap(), PipelineResult::Completed);
  let guard = ctx.read();
  assert!(!guard.scoped_a_ran);
  assert!(!guard.scoped_b_ran);
  assert_eq!(guard.scoped_result, None, "no scope ran, so nothing was merged");
}

#[tokio::test]
async fn test_conditional_no_match_behavior_continue() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["pre_cond", "conditional_step", "post_cond"]);
  pipeline.on_root("pre_cond", create_simple_handler("pre_cond", "PRE;"));

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(scoped_pipeline_a_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false)
    .add_dynamic_scope(scoped_pipeline_b_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false)
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false);

  pipeline.on_root("post_cond", create_simple_handler("post_cond", "POST;"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["pre_cond", "post_cond"]);
  assert_eq!(guard.message, "PRE;POST;");
}

#[tokio::test]
async fn test_conditional_no_match_behavior_stop() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["pre_cond", "conditional_step", "post_cond"]);
  pipeline.on_root("pre_cond", create_simple_handler("pre_cond", "PRE;"));

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(scoped_pipeline_a_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false)
    .if_no_scope_matches(PipelineControl::Stop)
    .finalize_conditional_step(false);

  pipeline.on_root("post_cond", create_simple_handler("post_cond", "POST;"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Stopped);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["pre_cond"]);
  assert_eq!(guard.message, "PRE;");
}

/// A conditional step must compose with, not clobber, handlers already registered on it.
#[tokio::test]
async fn test_conditional_scope_composes_with_existing_on_root_handler() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["shared_step"]);

  pipeline.on_root("shared_step", create_simple_handler("root_handler", "ROOT;"));

  pipeline
    .conditional_scopes_for_step("shared_step")
    .add_dynamic_scope(scoped_pipeline_a_factory(), |main_ctx: ContextData<TestContext>| {
      Ok(main_ctx.project(|d| ScopedTestContextA {
        input: d.data_for_scoped.clone().unwrap_or_default(),
        ..Default::default()
      }))
    })
    .with_merge(|main, _sub| main.scoped_a_ran = true)
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext {
    data_for_scoped: Some("x".to_string()),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["root_handler"], "on_root handler must still run");
  assert_eq!(guard.message, "ROOT;");
  assert!(guard.scoped_a_ran, "conditional scope must also run");
}

#[tokio::test]
async fn test_conditional_extractor_failure() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["conditional_step_fail_extract"]);

  pipeline
    .conditional_scopes_for_step("conditional_step_fail_extract")
    .add_dynamic_scope(scoped_pipeline_a_factory(), |_main_ctx: ContextData<TestContext>| {
      Err(OrkaError::ExtractorFailure {
        step_name: "test_extractor".to_string(),
        source: anyhow::anyhow!("Extractor failed intentionally"),
      })
    })
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  if let Err(TestError::Orka(s)) = result {
    assert!(s.contains("ExtractorFailure"));
    assert!(s.contains("Extractor failed intentionally"));
  } else {
    panic!("Expected TestError::Orka(ExtractorFailure), got {:?}", result);
  }
}

#[tokio::test]
async fn test_conditional_provider_failure() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["conditional_step_fail_provide"]);

  pipeline
    .conditional_scopes_for_step("conditional_step_fail_provide")
    .add_dynamic_scope(failing_provider_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  if let Err(TestError::Orka(s)) = result {
    assert!(s.contains("PipelineProviderFailure"));
    assert!(s.contains("Provider intentionally failed"));
  } else {
    panic!("Expected TestError::Orka(PipelineProviderFailure), got {:?}", result);
  }
}

#[tokio::test]
async fn test_conditional_static_scope_runs() {
  setup_tracing();

  let mut scoped_pipeline_static_a = Pipeline::<ScopedTestContextA, TestError>::new(["static_scoped_task"]);
  scoped_pipeline_static_a.on_root("static_scoped_task", |s_ctx: ContextData<ScopedTestContextA>| async move {
    let mut guard = s_ctx.write();
    guard.processed_message = format!("STATIC A processed: {}", guard.input);
    Ok(PipelineControl::Continue)
  });
  let arc_static_pipeline_a = Arc::new(scoped_pipeline_static_a);

  let mut main_pipeline = Pipeline::<TestContext, TestError>::new(["conditional_with_static"]);

  main_pipeline
    .conditional_scopes_for_step("conditional_with_static")
    .add_static_scope(
      arc_static_pipeline_a.clone(),
      |main_ctx: ContextData<TestContext>| {
        Ok(main_ctx.project(|d| ScopedTestContextA {
          input: d
            .data_for_scoped
            .clone()
            .unwrap_or_else(|| "default_static_input".to_string()),
          ..Default::default()
        }))
      },
    )
    .with_merge(|main, sub| {
      main.scoped_a_ran = true;
      main.scoped_result = Some(sub.processed_message.clone());
    })
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "use_static_a")
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext {
    message: "use_static_a".to_string(),
    data_for_scoped: Some("input_for_static_a".to_string()),
    ..Default::default()
  });
  let result = main_pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert!(guard.scoped_a_ran);
  assert_eq!(
    guard.scoped_result.as_deref(),
    Some("STATIC A processed: input_for_static_a")
  );
}

/// A scope that fails must not merge its partial state into the main context.
#[tokio::test]
async fn test_failed_scope_is_not_merged_back() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["cond"]);

  pipeline
    .conditional_scopes_for_step("cond")
    .add_dynamic_scope(failing_scoped_pipeline_factory("boom"), |_| Ok(ContextData::default()))
    .with_merge(|main, _sub| main.scoped_a_ran = true)
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err());
  assert!(
    !ctx.read().scoped_a_ran,
    "a failed scope must not run its merge function"
  );
}

#[tokio::test]
#[serial]
async fn test_conditional_scoped_pipeline_returns_error() {
  setup_tracing();
  reset_counters();

  let mut pipeline = Pipeline::<TestContext, TestError>::new(["conditional_step_with_failing_scope"]);
  let failure_msg = "Scoped pipeline intentionally failed!";

  pipeline
    .conditional_scopes_for_step("conditional_step_with_failing_scope")
    .add_dynamic_scope(
      counting_failing_scoped_pipeline_factory(failure_msg),
      |_main_ctx: ContextData<TestContext>| {
        EXTRACTOR_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(ContextData::new(ScopedTestContextA {
          input: "any_input".to_string(),
          ..Default::default()
        }))
      },
    )
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err(), "Pipeline run should have failed");
  match result.err().unwrap() {
    TestError::ScopedTask(msg) => assert_eq!(msg, failure_msg),
    other_err => panic!("Expected TestError::ScopedTask, got {:?}", other_err),
  }

  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 1, "provider ran once");
  assert_eq!(EXTRACTOR_A_EXEC_COUNTER.load(Ordering::SeqCst), 1, "extractor ran once");
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 1, "scoped handler ran once");
}

#[tokio::test]
#[serial]
async fn test_optional_conditional_step_continues_on_scope_error() {
  setup_tracing();
  reset_counters();

  let mut pipeline =
    Pipeline::<TestContext, TestError>::new(["before_optional_cond", "optional_conditional_step", "after_optional_cond"]);

  pipeline
    .optional("optional_conditional_step")
    .on_root("before_optional_cond", create_simple_handler("before_opt", "Before;"));

  pipeline
    .conditional_scopes_for_step("optional_conditional_step")
    .add_dynamic_scope(
      counting_failing_scoped_pipeline_factory("Scoped pipeline in optional step failed!"),
      |_| Ok(ContextData::default()),
    )
    .on_condition(|_| true)
    // `true` here marks the main step optional, so a scope error is swallowed.
    .finalize_conditional_step(true);

  pipeline.on_root("after_optional_cond", create_simple_handler("after_opt", "After;"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(
    result.is_ok(),
    "Pipeline should succeed despite optional step failure: {:?}",
    result.err()
  );
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["before_opt", "after_opt"]);
  assert_eq!(guard.message, "Before;After;");
  assert_eq!(guard.counter, 2);

  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[serial]
async fn test_optional_conditional_step_continues_on_provider_failure() {
  setup_tracing();
  reset_counters();

  let mut pipeline =
    Pipeline::<TestContext, TestError>::new(["before_opt_prov_fail", "opt_cond_step_prov_fail", "after_opt_prov_fail"]);

  pipeline
    .optional("opt_cond_step_prov_fail")
    .on_root("before_opt_prov_fail", create_simple_handler("before_opt_pf", "BPF;"));

  pipeline
    .conditional_scopes_for_step("opt_cond_step_prov_fail")
    .add_dynamic_scope(failing_provider_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| true)
    .finalize_conditional_step(true);

  pipeline.on_root("after_opt_prov_fail", create_simple_handler("after_opt_pf", "APF;"));

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run should have succeeded: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["before_opt_pf", "after_opt_pf"]);
  assert_eq!(guard.message, "BPF;APF;");
}

/// The counting factory exists only for the invocation-count assertions above; this keeps
/// it exercised so it does not rot.
#[tokio::test]
#[serial]
async fn test_counting_factory_reports_single_invocation() {
  setup_tracing();
  reset_counters();

  let mut pipeline = Pipeline::<TestContext, TestError>::new(["cond"]);
  pipeline
    .conditional_scopes_for_step("cond")
    .add_dynamic_scope(counting_scoped_pipeline_a_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| true)
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  pipeline.run(ctx).await.unwrap();

  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
}
