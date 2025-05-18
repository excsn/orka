// tests/conditional_scope_tests.rs
mod common;

use common::*;
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use serial_test::serial;
use std::sync::{atomic::Ordering, Arc};

// --- Helper Factories for Scoped Pipelines ---
fn create_scoped_pipeline_a_factory(
) -> impl Fn(ContextData<TestContext>) -> std::future::Ready<Result<Arc<Pipeline<ScopedTestContextA, TestError>>, OrkaError>>
{
  move |_main_ctx: ContextData<TestContext>| {
    PROVIDER_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(&[("scoped_a_task", false, None)]);
    p.on_root("scoped_a_task", |s_ctx: ContextData<ScopedTestContextA>| {
      Box::pin(async move {
        SCOPED_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut guard = s_ctx.write();
        guard.processed_message = format!("A processed: {}", guard.input);
        tracing::info!(target: "test_scoped", "Scoped A executed: {}", guard.processed_message);
        Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
      })
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn create_scoped_pipeline_b_factory(
) -> impl Fn(ContextData<TestContext>) -> std::future::Ready<Result<Arc<Pipeline<ScopedTestContextB, TestError>>, OrkaError>>
{
  move |_main_ctx: ContextData<TestContext>| {
    PROVIDER_B_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut p = Pipeline::<ScopedTestContextB, TestError>::new(&[("scoped_b_task", false, None)]);
    p.on_root("scoped_b_task", |s_ctx: ContextData<ScopedTestContextB>| {
      Box::pin(async move {
        SCOPED_B_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut guard = s_ctx.write();
        guard.alternative_message = format!("B alternative: {}", guard.input);
        tracing::info!(target: "test_scoped", "Scoped B executed: {}", guard.alternative_message);
        Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
      })
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

fn failing_provider_factory(
) -> impl Fn(ContextData<TestContext>) -> std::future::Ready<Result<Arc<Pipeline<ScopedTestContextA, TestError>>, OrkaError>>
{
  move |_main_ctx: ContextData<TestContext>| {
    std::future::ready(Err(OrkaError::PipelineProviderFailure {
      step_name: "failing_provider".to_string(),
      source: anyhow::anyhow!("Provider intentionally failed"),
    }))
  }
}

#[tokio::test]
#[serial]
async fn test_conditional_scope_a_runs_when_condition_met() {
  setup_tracing();
  reset_counters();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_step", false, None)]);

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(
      create_scoped_pipeline_a_factory(),
      |main_ctx: ContextData<TestContext>| {
        EXTRACTOR_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        let data = main_ctx.read().data_for_scoped.clone().unwrap_or_default();
        Ok(ContextData::new(ScopedTestContextA {
          input: data,
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "run_a")
    .add_dynamic_scope(
      create_scoped_pipeline_b_factory(),
      |main_ctx: ContextData<TestContext>| {
        EXTRACTOR_B_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        let data = main_ctx.read().data_for_scoped.clone().unwrap_or_default();
        Ok(ContextData::new(ScopedTestContextB {
          input: data,
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "run_b")
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext {
    message: "run_a".to_string(),
    data_for_scoped: Some("data_a".to_string()),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);
  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(EXTRACTOR_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(PROVIDER_B_EXEC_COUNTER.load(Ordering::SeqCst), 0); // B should not have run
  assert_eq!(EXTRACTOR_B_EXEC_COUNTER.load(Ordering::SeqCst), 0);
  assert_eq!(SCOPED_B_EXEC_COUNTER.load(Ordering::SeqCst), 0);

  // To check data from scoped pipeline, we'd need the extractor to merge data back or operate on shared parts.
  // For this test, we rely on execution counters and logs.
}

#[tokio::test]
#[serial]
async fn test_conditional_scope_b_runs_when_condition_met() {
  setup_tracing();
  reset_counters();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_step", false, None)]);
  // ... (similar setup as above, but make condition for B true) ...
  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(create_scoped_pipeline_a_factory(), |main_ctx| {
      EXTRACTOR_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
      Ok(ContextData::new(ScopedTestContextA {
        input: main_ctx.read().data_for_scoped.clone().unwrap_or_default(),
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx| main_ctx.read().message == "run_a")
    .add_dynamic_scope(create_scoped_pipeline_b_factory(), |main_ctx| {
      EXTRACTOR_B_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
      Ok(ContextData::new(ScopedTestContextB {
        input: main_ctx.read().data_for_scoped.clone().unwrap_or_default(),
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx| main_ctx.read().message == "run_b")
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext {
    message: "run_b".to_string(),
    data_for_scoped: Some("data_b".to_string()),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);
  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 0);
  assert_eq!(EXTRACTOR_A_EXEC_COUNTER.load(Ordering::SeqCst), 0);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 0);
  assert_eq!(PROVIDER_B_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(EXTRACTOR_B_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(SCOPED_B_EXEC_COUNTER.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[serial]
async fn test_conditional_no_match_behavior_continue() {
  setup_tracing();
  reset_counters();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("pre_cond", false, None),
    ("conditional_step", false, None),
    ("post_cond", false, None),
  ]);
  pipeline.on_root("pre_cond", create_simple_handler("pre_cond", "PRE;"));

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(create_scoped_pipeline_a_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false) // Condition A always false
    .add_dynamic_scope(create_scoped_pipeline_b_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false) // Condition B always false
    .if_no_scope_matches(PipelineControl::Continue) // Explicitly continue
    .finalize_conditional_step(false);

  pipeline.on_root("post_cond", create_simple_handler("post_cond", "POST;"));

  // Initialize with an empty message, or a message field that doesn't interfere with the expected outcome
  let ctx = ContextData::new(TestContext {
    message: String::new(), // CORRECTED: Start with an empty string
    // If 'no_match' was for setting up a specific condition that pre_cond or post_cond
    // checks, that's different. But here it just seems to be initial state.
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 0);
  assert_eq!(SCOPED_B_EXEC_COUNTER.load(Ordering::SeqCst), 0);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["pre_cond", "post_cond"]);
  assert_eq!(guard.message, "PRE;POST;"); // Now this should pass
}

#[tokio::test]
#[serial]
async fn test_conditional_no_match_behavior_stop() {
  setup_tracing();
  reset_counters();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("pre_cond", false, None),
    ("conditional_step", false, None),
    ("post_cond", false, None), // Should not run
  ]);
  pipeline.on_root("pre_cond", create_simple_handler("pre_cond", "PRE;"));

  pipeline
    .conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(create_scoped_pipeline_a_factory(), |_| Ok(ContextData::default()))
    .on_condition(|_| false)
    .if_no_scope_matches(PipelineControl::Stop) // Explicitly stop
    .finalize_conditional_step(false);

  pipeline.on_root("post_cond", create_simple_handler("post_cond", "POST;"));

  let ctx = ContextData::new(TestContext {
    message: String::new(), // CORRECTED: Start with an empty string
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;
  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Stopped); // Pipeline should be stopped

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["pre_cond"]); // Only pre_cond runs
  assert_eq!(guard.message, "PRE;"); // Now this should pass
}

#[tokio::test]
#[serial]
async fn test_conditional_extractor_failure() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_step_fail_extract", false, None)]);

  pipeline
    .conditional_scopes_for_step("conditional_step_fail_extract")
    .add_dynamic_scope(
      create_scoped_pipeline_a_factory(),
      |_main_ctx: ContextData<TestContext>| {
        Err(OrkaError::ExtractorFailure {
          // Extractor fails with OrkaError
          step_name: "test_extractor".to_string(),
          source: anyhow::anyhow!("Extractor failed intentionally"),
        })
      },
    )
    .on_condition(|_main_ctx: ContextData<TestContext>| true) // Condition met
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
#[serial]
async fn test_conditional_provider_failure() {
  setup_tracing();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_step_fail_provide", false, None)]);

  pipeline
    .conditional_scopes_for_step("conditional_step_fail_provide")
    .add_dynamic_scope(
      failing_provider_factory(), // This factory returns Err(OrkaError)
      |_main_ctx: ContextData<TestContext>| Ok(ContextData::default()), // Extractor is fine
    )
    .on_condition(|_main_ctx: ContextData<TestContext>| true) // Condition met
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

// --- Test for Static Scopes ---
#[tokio::test]
#[serial] // Keep serial if using global counters
async fn test_conditional_static_scope_runs() {
  setup_tracing();
  reset_counters();

  // 1. Create the static scoped pipeline instance
  let mut scoped_pipeline_static_a =
    Pipeline::<ScopedTestContextA, TestError>::new(&[("static_scoped_task", false, None)]);
  scoped_pipeline_static_a.on_root("static_scoped_task", |s_ctx: ContextData<ScopedTestContextA>| {
    Box::pin(async move {
      SCOPED_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst); // Use a shared counter
      let mut guard = s_ctx.write();
      guard.processed_message = format!("STATIC A processed: {}", guard.input);
      tracing::info!(target: "test_scoped_static", "Static Scoped A executed: {}", guard.processed_message);
      Ok::<_, TestError>(PipelineControl::Continue)
    })
  });
  let arc_static_pipeline_a = Arc::new(scoped_pipeline_static_a);

  // 2. Create the main pipeline
  let mut main_pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_with_static", false, None)]);

  main_pipeline
    .conditional_scopes_for_step("conditional_with_static")
    .add_static_scope(
      // Use add_static_scope
      arc_static_pipeline_a.clone(), // Pass the Arc'd pipeline
      |main_ctx: ContextData<TestContext>| {
        // Extractor
        EXTRACTOR_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        let data = main_ctx
          .read()
          .data_for_scoped
          .clone()
          .unwrap_or_else(|| "default_static_input".to_string());
        Ok(ContextData::new(ScopedTestContextA {
          input: data,
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<TestContext>| main_ctx.read().message == "use_static_a") // Condition
    .finalize_conditional_step(false);

  // 3. Run and Assert
  let ctx = ContextData::new(TestContext {
    message: "use_static_a".to_string(),
    data_for_scoped: Some("input_for_static_a".to_string()),
    ..Default::default()
  });
  let result = main_pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run failed: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  // Assert that the static scoped pipeline's handler ran
  assert_eq!(
    EXTRACTOR_A_EXEC_COUNTER.load(Ordering::SeqCst),
    1,
    "Extractor for static scope A did not run expected times"
  );
  assert_eq!(
    SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst),
    1,
    "Static scope A task did not run expected times"
  );

  // Optionally, if the scoped pipeline modifies its context and the extractor shares state
  // (or you merge it back in an after_root), you could assert on that context.
  // For this example, execution count is sufficient.
}

// --- Helper Factory for a Scoped Pipeline that Fails ---
fn create_failing_scoped_pipeline_factory(
  failure_message: &'static str,
) -> impl Fn(ContextData<TestContext>) -> std::future::Ready<Result<Arc<Pipeline<ScopedTestContextA, TestError>>, OrkaError>>
{
  let owned_failure_message = failure_message.to_string();
  move |_main_ctx: ContextData<TestContext>| {
    PROVIDER_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst); // Count provider execution
    let mut p = Pipeline::<ScopedTestContextA, TestError>::new(&[("failing_scoped_task", false, None)]);
    let msg_clone = owned_failure_message.clone();
    p.on_root("failing_scoped_task", move |s_ctx: ContextData<ScopedTestContextA>| {
      let s_msg_clone = msg_clone.clone();
      Box::pin(async move {
        SCOPED_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst); // Count handler execution
        tracing::warn!(target: "test_scoped_fail", "Scoped task is about to fail: {}", s_msg_clone);
        Err(TestError::ScopedTask(s_msg_clone)) // Specific error variant for scoped task
      })
    });
    std::future::ready(Ok(Arc::new(p)))
  }
}

// Need to add ScopedTask to TestError enum in common.rs
// // tests/common/mod.rs
// #[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
// pub enum TestError {
//     // ... existing ...
//     #[error("Test scoped task failed: {0}")]
//     ScopedTask(String), // New variant
// }

#[tokio::test]
#[serial]
async fn test_conditional_scoped_pipeline_returns_error() {
  setup_tracing();
  reset_counters();

  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[("conditional_step_with_failing_scope", false, None)]);
  let failure_msg = "Scoped pipeline intentionally failed!";

  pipeline
    .conditional_scopes_for_step("conditional_step_with_failing_scope")
    .add_dynamic_scope(
      create_failing_scoped_pipeline_factory(failure_msg),
      |main_ctx: ContextData<TestContext>| {
        // Extractor
        EXTRACTOR_A_EXEC_COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(ContextData::new(ScopedTestContextA {
          input: "any_input".to_string(),
          ..Default::default()
        }))
      },
    )
    .on_condition(|_main_ctx: ContextData<TestContext>| true) // Always run this scope
    .finalize_conditional_step(false);

  let ctx = ContextData::new(TestContext::default());
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_err(), "Pipeline run should have failed");
  match result.err().unwrap() {
    TestError::ScopedTask(msg) => {
      assert_eq!(msg, failure_msg);
    }
    other_err => panic!("Expected TestError::ScopedTask, got {:?}", other_err),
  }

  assert_eq!(
    PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst),
    1,
    "Failing provider was not called"
  );
  assert_eq!(
    EXTRACTOR_A_EXEC_COUNTER.load(Ordering::SeqCst),
    1,
    "Extractor for failing scope was not called"
  );
  assert_eq!(
    SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst),
    1,
    "Failing scoped task handler was not called"
  );
}

#[tokio::test]
#[serial]
async fn test_optional_conditional_step_continues_on_scope_error() {
  setup_tracing();
  reset_counters();

  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("before_optional_cond", false, None),
    ("optional_conditional_step", true, None), // Main step is optional
    ("after_optional_cond", false, None),
  ]);

  pipeline.on_root("before_optional_cond", create_simple_handler("before_opt", "Before;"));

  let failure_msg = "Scoped pipeline in optional step failed!";
  pipeline
    .conditional_scopes_for_step("optional_conditional_step")
    .add_dynamic_scope(
      create_failing_scoped_pipeline_factory(failure_msg), // This scope will run and fail
      |_main_ctx: ContextData<TestContext>| Ok(ContextData::default()),
    )
    .on_condition(|_main_ctx: ContextData<TestContext>| true)
    .finalize_conditional_step(true); // *** Mark the conditional step as optional ***

  pipeline.on_root("after_optional_cond", create_simple_handler("after_opt", "After;"));

  let ctx = ContextData::new(TestContext {
    message: String::new(),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  // The pipeline should complete because the failing conditional step was optional
  assert!(
    result.is_ok(),
    "Pipeline run should have succeeded despite optional step failure: {:?}",
    result.err()
  );
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  // Check that all main steps ran
  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["before_opt", "after_opt"]);
  assert_eq!(guard.message, "Before;After;");
  assert_eq!(guard.counter, 2);

  // Check that the failing scope's components were indeed attempted
  assert_eq!(PROVIDER_A_EXEC_COUNTER.load(Ordering::SeqCst), 1);
  assert_eq!(SCOPED_A_EXEC_COUNTER.load(Ordering::SeqCst), 1); // The failing handler itself
}

#[tokio::test]
#[serial]
async fn test_optional_conditional_step_continues_on_provider_failure() {
  setup_tracing();
  reset_counters();

  let mut pipeline = Pipeline::<TestContext, TestError>::new(&[
    ("before_opt_prov_fail", false, None),
    ("opt_cond_step_prov_fail", true, None), // Main step is optional
    ("after_opt_prov_fail", false, None),
  ]);

  pipeline.on_root("before_opt_prov_fail", create_simple_handler("before_opt_pf", "BPF;"));

  pipeline
    .conditional_scopes_for_step("opt_cond_step_prov_fail")
    .add_dynamic_scope(
      failing_provider_factory(), // This provider will fail
      |_main_ctx: ContextData<TestContext>| Ok(ContextData::default()),
    )
    .on_condition(|_main_ctx: ContextData<TestContext>| true)
    .finalize_conditional_step(true); // *** Mark the conditional step as optional ***

  pipeline.on_root("after_opt_prov_fail", create_simple_handler("after_opt_pf", "APF;"));

  let ctx = ContextData::new(TestContext {
    message: String::new(),
    ..Default::default()
  });
  let result = pipeline.run(ctx.clone()).await;

  assert!(result.is_ok(), "Pipeline run should have succeeded: {:?}", result.err());
  assert_eq!(result.unwrap(), PipelineResult::Completed);

  let guard = ctx.read();
  assert_eq!(guard.steps_executed, vec!["before_opt_pf", "after_opt_pf"]);
  assert_eq!(guard.message, "BPF;APF;");
}
