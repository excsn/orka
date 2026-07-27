//! Tests for `resolve_plan` and the step-isolation runners `run_step`/`run_from`/
//! `run_until`, including their contractual trace shape (step events only, no
//! `RunStarted`/`RunFinished`).

mod common;

use common::{setup_tracing, TestContext, TestError};
use orka::test_util::noop_pipeline;
use orka::{
  ContextData, Pipeline, PipelineControl, PipelineResult, PlannedAction, SkipReason, StepPlan, TraceCollector,
  TraceEventKind,
};

fn recording_pipeline() -> Pipeline<TestContext, TestError> {
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["load", "process", "notify"]);
  for name in ["load", "process", "notify"] {
    p.on_root(name, move |ctx| async move {
      ctx.write().steps_executed.push(name.to_string());
      Ok(PipelineControl::Continue)
    });
  }
  p
}

#[test]
fn resolve_plan_reports_run_skip_and_missing_per_seeded_context() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = noop_pipeline(["always", "conditional", "empty_optional"]);
  p.clear_on("empty_optional");
  p.optional("empty_optional");
  p.skip_if("conditional", |ctx| ctx.read().counter > 10);

  // Table test over seeded contexts; nothing executes.
  let low = ContextData::new(TestContext {
    counter: 1,
    ..TestContext::default()
  });
  assert_eq!(
    p.resolve_plan(&low),
    vec![
      StepPlan {
        name: "always".into(),
        action: PlannedAction::Run
      },
      StepPlan {
        name: "conditional".into(),
        action: PlannedAction::Run
      },
      StepPlan {
        name: "empty_optional".into(),
        action: PlannedAction::Skip(SkipReason::OptionalWithoutHandlers)
      },
    ]
  );

  let high = ContextData::new(TestContext {
    counter: 42,
    ..TestContext::default()
  });
  assert_eq!(
    p.resolve_plan(&high)[1].action,
    PlannedAction::Skip(SkipReason::SkipCondition { label: None })
  );

  // A required, handler-less step is predicted as the failure `run` would produce.
  let mut broken: Pipeline<TestContext, TestError> = Pipeline::new(["nothing_here"]);
  broken.skip_if("nothing_here", |_ctx| false);
  assert_eq!(
    broken.resolve_plan(&ContextData::new(TestContext::default()))[0].action,
    PlannedAction::FailMissingHandlers
  );
}

#[test]
fn resolve_plan_executes_no_handlers() {
  setup_tracing();
  let p = recording_pipeline();
  let ctx = ContextData::new(TestContext::default());
  let plan = p.resolve_plan(&ctx);
  assert_eq!(plan.len(), 3);
  assert!(ctx.read().steps_executed.is_empty());
}

#[tokio::test]
async fn run_step_executes_exactly_one_step() {
  setup_tracing();
  let p = recording_pipeline();
  let ctx = ContextData::new(TestContext::default());
  let result = p.run_step("process", ctx.clone()).await.unwrap();
  assert_eq!(result, PipelineResult::Completed);
  assert_eq!(ctx.read().steps_executed, vec!["process"]);
}

#[tokio::test]
async fn run_step_still_respects_skip_if() {
  setup_tracing();
  let mut p = recording_pipeline();
  p.skip_if("process", |_ctx| true);
  let ctx = ContextData::new(TestContext::default());
  p.run_step("process", ctx.clone()).await.unwrap();
  assert!(ctx.read().steps_executed.is_empty());
}

#[tokio::test]
async fn run_from_and_run_until_are_inclusive_ranges() {
  setup_tracing();
  let p = recording_pipeline();

  let ctx = ContextData::new(TestContext::default());
  p.run_from("process", ctx.clone()).await.unwrap();
  assert_eq!(ctx.read().steps_executed, vec!["process", "notify"]);

  let ctx2 = ContextData::new(TestContext::default());
  p.run_until("process", ctx2.clone()).await.unwrap();
  assert_eq!(ctx2.read().steps_executed, vec!["load", "process"]);
}

#[tokio::test]
async fn partial_runners_error_on_unknown_step() {
  setup_tracing();
  let p = recording_pipeline();
  let ctx = ContextData::new(TestContext::default());
  for result in [
    p.run_step("nope", ctx.clone()).await,
    p.run_from("nope", ctx.clone()).await,
    p.run_until("nope", ctx.clone()).await,
  ] {
    let err = result.unwrap_err();
    assert!(matches!(&err, TestError::Orka(msg) if msg.contains("StepNotFound")), "got: {:?}", err);
  }
}

#[tokio::test]
async fn partial_run_trace_is_step_events_only() {
  setup_tracing();
  // Contractual: partial runs are not runs, so they emit no RunStarted/RunFinished, but
  // their step events are still run_id-tagged for for_run filtering.
  let p = recording_pipeline();
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  p.run_step("process", ContextData::new(TestContext::default())).await.unwrap();

  let events = trace.events();
  assert!(!events.is_empty());
  assert!(events.iter().all(|e| !matches!(
    e.kind,
    TraceEventKind::RunStarted | TraceEventKind::RunFinished { .. }
  )));
  assert_eq!(trace.run_count(), 0);

  let run_ids = trace.run_ids();
  assert_eq!(run_ids.len(), 1);
  let run = trace.for_run(run_ids[0]);
  assert_eq!(run.completed_steps(), vec!["process"]);

  // Step indexes reflect the position in the full pipeline, not the slice.
  assert!(events.iter().any(|e| matches!(
    e.kind,
    TraceEventKind::StepStarted { ref step, index: 1 } if step == "process"
  )));
}

#[test]
fn skip_if_labeled_shows_its_label_in_resolve_plan() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = noop_pipeline(["drain", "install"]);
  p.skip_if_labeled("drain", "drain disabled by config", |ctx| ctx.read().counter == 0);

  let plan = p.resolve_plan(&ContextData::new(TestContext::default()));
  assert_eq!(
    plan[0].action,
    PlannedAction::Skip(SkipReason::SkipCondition {
      label: Some("drain disabled by config".to_string())
    })
  );

  // Re-registering an unlabeled skip_if clears the stale label; clear_skip_condition too.
  p.skip_if("drain", |_ctx| true);
  assert_eq!(
    p.resolve_plan(&ContextData::new(TestContext::default()))[0].action,
    PlannedAction::Skip(SkipReason::SkipCondition { label: None })
  );
}

#[test]
fn step_plan_renders_a_readable_preview() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = noop_pipeline(["prepare", "drain", "cleanup", "install"]);
  p.skip_if_labeled("drain", "drain disabled by config", |_ctx| true);
  p.skip_if("cleanup", |_ctx| true);
  p.clear_on("install"); // required step left with no handlers

  let plan = p.resolve_plan(&ContextData::new(TestContext::default()));
  let rendered: Vec<String> = plan.iter().map(|step| step.to_string()).collect();
  assert_eq!(
    rendered,
    vec![
      "prepare: run",
      "drain: skip (drain disabled by config)",
      "cleanup: skip (skip_if condition)",
      "install: fail (required step has no handlers)",
    ]
  );
}
