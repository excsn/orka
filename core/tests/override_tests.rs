//! Tests for the handler override surface: surgical per-phase clear/replace, the
//! orphaned-extractor validate failure after `clear_on`, and `stub_step` on plain and
//! conditional steps.

mod common;

use common::{setup_tracing, ScopedTestContextA, SubExtractContext, TestContext, TestError};
use orka::test_util::{assert_steps_completed, ExecutionCounter, PipelineTestExt};
use orka::{ContextData, Pipeline, PipelineControl, PipelineResult, StepPhase, TraceCollector};
use std::sync::Arc;

#[tokio::test]
async fn replace_per_phase_is_surgical() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["work"]);
  p.before_root("work", |ctx| async move {
    ctx.write().steps_executed.push("before-original".into());
    Ok(PipelineControl::Continue)
  })
  .on_root("work", |ctx| async move {
    ctx.write().steps_executed.push("on-original".into());
    Ok(PipelineControl::Continue)
  })
  .after_root("work", |ctx| async move {
    ctx.write().steps_executed.push("after-original".into());
    Ok(PipelineControl::Continue)
  });

  // Replacing `on` must leave before/after untouched.
  p.replace_on_root("work", |ctx| async move {
    ctx.write().steps_executed.push("on-replacement".into());
    Ok(PipelineControl::Continue)
  });

  let ctx = ContextData::new(TestContext::default());
  p.run(ctx.clone()).await.unwrap();
  assert_eq!(
    ctx.read().steps_executed,
    vec!["before-original", "on-replacement", "after-original"]
  );

  // And the other two replacements are equally surgical.
  p.replace_before_root("work", |ctx| async move {
    ctx.write().steps_executed.push("before-replacement".into());
    Ok(PipelineControl::Continue)
  })
  .replace_after_root("work", |ctx| async move {
    ctx.write().steps_executed.push("after-replacement".into());
    Ok(PipelineControl::Continue)
  });
  let ctx2 = ContextData::new(TestContext::default());
  p.run(ctx2.clone()).await.unwrap();
  assert_eq!(
    ctx2.read().steps_executed,
    vec!["before-replacement", "on-replacement", "after-replacement"]
  );
}

#[tokio::test]
async fn replace_collapses_multiple_handlers_to_one() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["multi"]);
  let counter = ExecutionCounter::new();
  for _ in 0..3 {
    let c = counter.clone();
    p.on_root("multi", move |_ctx| {
      let c = c.clone();
      async move {
        c.increment();
        Ok(PipelineControl::Continue)
      }
    });
  }
  p.replace_on_root("multi", |_ctx| async { Ok(PipelineControl::Continue) });

  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_eq!(counter.get(), 0, "all three originals must be gone");
  assert!(p.has_handlers("multi", StepPhase::On));
}

#[tokio::test]
async fn clear_phase_methods_empty_only_their_phase() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["work"]);
  p.before_root("work", |_ctx| async { Ok(PipelineControl::Continue) })
    .on_root("work", |_ctx| async { Ok(PipelineControl::Continue) })
    .after_root("work", |_ctx| async { Ok(PipelineControl::Continue) });

  p.clear_before("work");
  assert!(!p.has_handlers("work", StepPhase::Before));
  assert!(p.has_handlers("work", StepPhase::On));
  assert!(p.has_handlers("work", StepPhase::After));

  p.clear_after("work");
  assert!(p.has_handlers("work", StepPhase::On));
  assert!(!p.has_handlers("work", StepPhase::After));

  p.clear_on("work");
  assert!(!p.has_handlers("work", StepPhase::On));

  // All phases now empty on a required step: validate reports it.
  assert!(p.validate().is_err());
}

#[tokio::test]
async fn clear_on_orphaning_an_extractor_fails_validate_loudly() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["extract"]);
  p.set_extractor("extract", |_main| Ok(ContextData::new(SubExtractContext::default())));
  p.on("extract", |sub: ContextData<SubExtractContext>| async move {
    sub.write().processed = true;
    Ok(PipelineControl::Continue)
  });
  assert!(p.validate().is_ok());

  // Surgical clear_on drops the consumer but NOT the extractor: the orphan is a real
  // configuration problem and validate must say so.
  p.clear_on("extract");
  let err = p.validate().unwrap_err();
  assert!(err.to_string().contains("extractor"), "unexpected error: {}", err);

  // remove_extractor resolves it (step still needs a handler to be valid).
  p.remove_extractor("extract");
  p.on_root("extract", |_ctx| async { Ok(PipelineControl::Continue) });
  assert!(p.validate().is_ok());
}

#[tokio::test]
async fn stub_step_neutralizes_a_conditional_step() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["branch", "tail"]);
  p.on_root("tail", |_ctx| async { Ok(PipelineControl::Continue) });

  // Conditional scope that would fail the run if it ever executed.
  let mut scoped: Pipeline<ScopedTestContextA, TestError> = Pipeline::new(["scoped_work"]);
  scoped.fail_at("scoped_work", || TestError::ScopedTask("must not run".into()));
  p.conditional_scopes_for_step("branch")
    .add_static_scope(Arc::new(scoped), |_main| {
      Ok(ContextData::new(ScopedTestContextA::default()))
    })
    .on_condition(|_ctx| true)
    .finalize_conditional_step(false);

  // Sanity: unstubbed, the run fails through the conditional scope.
  assert!(p.run(ContextData::new(TestContext::default())).await.is_err());

  // stub_step drops the master handler with everything else and installs a Continue stub;
  // the step still shows as completed in the trace and validate stays green.
  p.stub_step("branch");
  assert!(p.validate().is_ok());
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());
  let result = p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_eq!(result, PipelineResult::Completed);
  assert_steps_completed(&trace, &["branch", "tail"]);
}

#[tokio::test]
async fn stub_step_also_clears_extractor_bookkeeping() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["extract"]);
  p.set_extractor("extract", |_main| Ok(ContextData::new(SubExtractContext::default())));
  p.on("extract", |_sub: ContextData<SubExtractContext>| async {
    Ok(PipelineControl::Continue)
  });

  p.stub_step("extract");
  // No orphaned-extractor complaint: stub_step removed the extractor too.
  assert!(p.validate().is_ok());
  p.run(ContextData::new(TestContext::default())).await.unwrap();
}

#[tokio::test]
async fn fail_at_forces_failure_at_the_named_step() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["a", "b", "c"]);
  for name in ["a", "b", "c"] {
    p.on_root(name, |ctx| async move {
      ctx.write().counter += 1;
      Ok(PipelineControl::Continue)
    });
  }
  p.fail_at("b", || TestError::Handler("injected".into()));

  let ctx = ContextData::new(TestContext::default());
  let err = p.run(ctx.clone()).await.unwrap_err();
  assert_eq!(err, TestError::Handler("injected".into()));
  // "a" ran, "b" was replaced (its original increment is gone), "c" never ran.
  assert_eq!(ctx.read().counter, 1);
}

#[test]
#[should_panic(expected = "not found in pipeline definition")]
fn override_methods_panic_on_unknown_step() {
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["known"]);
  p.clear_on("unknown");
}

/// A custom `AnyContextDataExtractor` injected through `set_extractor_impl`: the
/// injection seam that `set_extractor`/`set_extractor_with_merge` are concrete cases of.
#[tokio::test]
async fn set_extractor_impl_accepts_a_custom_extractor() {
  setup_tracing();

  struct RecordingExtractor {
    calls: ExecutionCounter,
  }
  impl orka::AnyContextDataExtractor<TestContext> for RecordingExtractor {
    fn extract_sub_context_data(
      &self,
      _root: ContextData<TestContext>,
    ) -> orka::OrkaResult<Box<dyn std::any::Any + Send>> {
      self.calls.increment();
      Ok(Box::new(ContextData::new(SubExtractContext {
        sub_field: "from custom extractor".into(),
        processed: false,
      })))
    }

    fn sub_context_data_type_id(&self) -> std::any::TypeId {
      std::any::TypeId::of::<SubExtractContext>()
    }
  }

  let calls = ExecutionCounter::new();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["extract"]);
  p.set_extractor_impl("extract", Arc::new(RecordingExtractor { calls: calls.clone() }));
  let seen = ContextData::new(String::new());
  let seen_in_handler = seen.clone();
  p.on("extract", move |sub: ContextData<SubExtractContext>| {
    let seen = seen_in_handler.clone();
    async move {
      *seen.write() = sub.read().sub_field.clone();
      Ok(PipelineControl::Continue)
    }
  });

  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_eq!(calls.get(), 1, "custom extractor was consulted");
  assert_eq!(*seen.read(), "from custom extractor");
}
