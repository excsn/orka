//! Proves every step-name parameter accepts a typed key, not just `&str`.
//!
//! The pattern this enables: name each step once in an enum, then let the compiler catch
//! typos and carry renames, instead of repeating a string literal at every registration,
//! skip condition, and override site.

mod common;

use common::{setup_tracing, TestContext, TestError};
use orka::prelude::*;
use orka::test_util::PipelineTestExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
  Prepare,
  Drain,
  Install,
  Verify,
}

impl Step {
  const ALL: [Step; 4] = [Step::Prepare, Step::Drain, Step::Install, Step::Verify];
}

impl AsRef<str> for Step {
  fn as_ref(&self) -> &str {
    match self {
      Step::Prepare => "prepare",
      Step::Drain => "drain",
      Step::Install => "install",
      Step::Verify => "verify",
    }
  }
}

fn record(name: &'static str) -> impl Fn(ContextData<TestContext>) -> std::future::Ready<Result<PipelineControl, TestError>> {
  move |ctx| {
    ctx.write().steps_executed.push(name.to_string());
    std::future::ready(Ok(PipelineControl::Continue))
  }
}

#[tokio::test]
async fn typed_keys_work_across_the_whole_step_api() {
  setup_tracing();

  // Construction already accepted AsRef<str>; now every other site does too.
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(Step::ALL);

  p.on_root(Step::Prepare, record("prepare"))
    .on_root(Step::Drain, record("drain"))
    .on_root(Step::Install, record("install"))
    .skip_if_labeled(Step::Drain, "drain disabled by config", |ctx| ctx.read().counter == 0)
    .must_precede(Step::Prepare, Step::Install)
    .optional(Step::Verify)
    .stub_step(Step::Verify);

  assert!(p.validate().is_ok());
  assert!(p.has_handlers(Step::Prepare, StepPhase::On));
  assert!(!p.has_handlers(Step::Prepare, StepPhase::Before));

  // The dry run reports the labeled skip against the typed key's name.
  let ctx = ContextData::new(TestContext::default());
  let plan = p.resolve_plan(&ctx);
  assert_eq!(plan[1].name, Step::Drain.as_ref());
  assert_eq!(
    plan[1].action,
    PlannedAction::Skip(SkipReason::SkipCondition {
      label: Some("drain disabled by config".to_string())
    })
  );

  p.run(ctx.clone()).await.unwrap();
  assert_eq!(ctx.read().steps_executed, vec!["prepare", "install"]);

  // Partial runners and the test-util extensions take them as well.
  let isolated = ContextData::new(TestContext::default());
  p.run_step(Step::Install, isolated.clone()).await.unwrap();
  assert_eq!(isolated.read().steps_executed, vec!["install"]);

  p.fail_at(Step::Install, || TestError::Handler("injected".to_string()));
  let failing = ContextData::new(TestContext {
    counter: 1, // drain no longer skipped
    ..TestContext::default()
  });
  let (result, outcome) = p.run_with_outcome(failing).await;
  assert_eq!(result.unwrap_err(), TestError::Handler("injected".to_string()));
  assert!(matches!(outcome, RunOutcome::Errored { ref step, .. } if step == Step::Install.as_ref()));
}

#[tokio::test]
async fn typed_keys_work_for_structural_edits_and_mix_with_strings() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new([Step::Prepare, Step::Install]);

  p.on_root(Step::Prepare, record("prepare"))
    .on_root(Step::Install, record("install"))
    .insert_before_step(Step::Install, Step::Drain)
    .on_root(Step::Drain, record("drain"))
    .insert_after_step(Step::Install, Step::Verify)
    .optional(Step::Verify);

  assert_eq!(
    p.step_names(),
    vec!["prepare", "drain", "install", "verify"],
    "inserts land relative to the typed keys"
  );

  // &str and String still work everywhere, and mix freely with typed keys: nothing about
  // the old call sites changed.
  p.clear_on("drain")
    .replace_on_root(String::from("drain"), record("drain (replaced)"))
    .required(Step::Verify)
    .stub_step("verify");

  let ctx = ContextData::new(TestContext::default());
  p.run(ctx.clone()).await.unwrap();
  assert_eq!(
    ctx.read().steps_executed,
    vec!["prepare", "drain (replaced)", "install"]
  );

  p.remove_step(Step::Drain);
  assert_eq!(p.step_names(), vec!["prepare", "install", "verify"]);
}

/// Resource names are `AsRef<str>` as well, so the dependency declarations take a typed
/// key on both sides and the comment becomes the code.
#[derive(Debug, Clone, Copy)]
enum Res {
  Release,
}

impl AsRef<str> for Res {
  fn as_ref(&self) -> &str {
    match self {
      Res::Release => "release",
    }
  }
}

#[tokio::test]
async fn typed_keys_work_for_resource_dependencies() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(Step::ALL);
  for step in Step::ALL {
    p.on_root(step, record("step"));
  }

  p.produces(Step::Prepare, Res::Release)
    .consumed_by(Res::Release, [Step::Drain, Step::Install])
    .must_precede_all(Step::Prepare, [Step::Install, Step::Verify]);

  assert!(p.validate().is_ok());

  // Reorder so a consumer runs before the producer: caught at validate, not at run time.
  p.remove_step(Step::Drain);
  p.insert_before_step(Step::Prepare, Step::Drain);
  p.on_root(Step::Drain, record("drain"));

  let err = p.validate().unwrap_err().to_string();
  assert!(err.contains("resource 'release'"), "error was: {}", err);
  assert!(err.contains("runs earlier"), "error was: {}", err);
}
