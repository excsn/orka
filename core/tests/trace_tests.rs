//! Tests for the observer/trace machinery: event streams, `&self` attachment (including
//! through an `Arc`), attachment before and after conditional finalization, run-id
//! disambiguation of concurrent runs, and `on_handler_error` type-level assertions.

mod common;

use common::{setup_tracing, ScopedTestContextA, TestContext, TestError};
use orka::test_util::{assert_order, assert_run_outcome, assert_steps_completed, assert_steps_skipped, PipelineTestExt};
use orka::{
  CompositeObserver, ContextData, HandlerOutcome, Pipeline, PipelineControl, PipelineObserver, RunOutcome, SkipReason,
  StepPhase, TraceCollector, TraceEvent, TraceEventKind,
};
use std::sync::Arc;

fn traced_pipeline() -> (Pipeline<TestContext, TestError>, TraceCollector) {
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["one", "two", "three"]);
  for name in ["one", "two", "three"] {
    p.on_root(name, |ctx| async move {
      ctx.write().counter += 1;
      Ok(PipelineControl::Continue)
    });
  }
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());
  (p, trace)
}

#[tokio::test]
async fn completed_run_emits_full_event_stream() {
  setup_tracing();
  let (p, trace) = traced_pipeline();
  p.run(ContextData::new(TestContext::default())).await.unwrap();

  assert_steps_completed(&trace, &["one", "two", "three"]);
  assert_order(&trace, &["one", "three"]);
  assert_run_outcome(&trace, RunOutcome::Completed);
  assert_eq!(trace.run_count(), 1);

  let events = trace.events();
  assert!(matches!(events.first().unwrap().kind, TraceEventKind::RunStarted));
  assert!(matches!(events.last().unwrap().kind, TraceEventKind::RunFinished { .. }));
  // Every step contributes Started, one HandlerFinished, Completed.
  assert_eq!(
    trace.handler_finishes("two", StepPhase::On),
    vec![HandlerOutcome::Continue]
  );
}

#[tokio::test]
async fn stopped_and_errored_runs_report_their_outcomes() {
  setup_tracing();
  let (mut p, trace) = traced_pipeline();
  p.replace_on_root("two", |_ctx| async { Ok(PipelineControl::Stop) });
  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_run_outcome(&trace, RunOutcome::Stopped);
  assert_eq!(
    trace.handler_finishes("two", StepPhase::On),
    vec![HandlerOutcome::Stop]
  );
  // "three" never started.
  assert!(!trace.step_completed("three"));

  let (mut p2, trace2) = traced_pipeline();
  p2.replace_on_root("two", |_ctx| async { Err(TestError::Handler("kaput".into())) });
  let err = p2.run(ContextData::new(TestContext::default())).await.unwrap_err();
  assert_eq!(err, TestError::Handler("kaput".into()));
  match trace2.last_outcome().unwrap() {
    RunOutcome::Errored { step, message } => {
      assert_eq!(step, "two");
      assert!(message.contains("kaput"));
    }
    other => panic!("expected Errored, got {:?}", other),
  }
}

#[tokio::test]
async fn skips_are_reported_with_their_reason() {
  setup_tracing();
  let (mut p, trace) = traced_pipeline();
  p.skip_if("one", |_ctx| true);
  p.clear_on("three");
  p.optional("three");
  p.run(ContextData::new(TestContext::default())).await.unwrap();

  assert_steps_skipped(&trace, &["one", "three"]);
  assert_steps_completed(&trace, &["two"]);
  let reasons: Vec<SkipReason> = trace
    .events()
    .into_iter()
    .filter_map(|e| match e.kind {
      TraceEventKind::StepSkipped { reason, .. } => Some(reason),
      _ => None,
    })
    .collect();
  assert_eq!(
    reasons,
    vec![
      SkipReason::SkipCondition { label: None },
      SkipReason::OptionalWithoutHandlers
    ]
  );
}

#[tokio::test]
async fn observer_attaches_through_a_shared_arc() {
  setup_tracing();
  // The registry model: the pipeline is behind an Arc, no &mut available. Attachment is
  // &self via the interior-mutable slot.
  let (p, _ignored) = traced_pipeline();
  let p = Arc::new(p);
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());
  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_steps_completed(&trace, &["one", "two", "three"]);
}

#[tokio::test]
async fn tracer_attached_after_conditional_finalization_still_sees_scope_events() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["branch"]);

  let mut scoped: Pipeline<ScopedTestContextA, TestError> = Pipeline::new(["scoped_work"]);
  scoped.on_root("scoped_work", |_ctx| async { Ok(PipelineControl::Continue) });

  p.conditional_scopes_for_step("branch")
    .add_static_scope(Arc::new(scoped), |_main| {
      Ok(ContextData::new(ScopedTestContextA::default()))
    })
    .on_condition(|ctx| ctx.read().counter > 0)
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false);

  // Attach AFTER finalize: the master handler captured the shared slot, so it must still
  // see this tracer.
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  // Matching context: counter > 0.
  let run_ctx = ContextData::new(TestContext {
    counter: 1,
    ..TestContext::default()
  });
  p.run(run_ctx).await.unwrap();
  let matched: Vec<_> = trace
    .events()
    .into_iter()
    .filter(|e| matches!(e.kind, TraceEventKind::ScopeMatched { .. }))
    .collect();
  assert_eq!(matched.len(), 1);
  // The scope event carries the same run id as the surrounding run.
  let run_id = trace.run_ids()[0];
  assert!(matched.iter().all(|e| e.run_id == run_id));

  // Non-matching context: ScopeNotMatched.
  trace.clear();
  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert!(trace
    .events()
    .iter()
    .any(|e| matches!(e.kind, TraceEventKind::ScopeNotMatched { .. })));
}

#[tokio::test]
async fn concurrent_runs_disambiguate_by_run_id() {
  setup_tracing();
  let (p, trace) = traced_pipeline();
  let p = Arc::new(p);

  let mut handles = Vec::new();
  for _ in 0..4 {
    let p = p.clone();
    handles.push(tokio::spawn(async move {
      p.run(ContextData::new(TestContext::default())).await.unwrap()
    }));
  }
  for h in handles {
    h.await.unwrap();
  }

  let run_ids = trace.run_ids();
  assert_eq!(run_ids.len(), 4);
  assert_eq!(trace.run_count(), 4);
  for run_id in run_ids {
    let run = trace.for_run(run_id);
    assert_eq!(run.completed_steps(), vec!["one", "two", "three"]);
    assert_eq!(run.last_outcome(), Some(RunOutcome::Completed));
  }
}

#[tokio::test]
async fn on_handler_error_sees_the_live_typed_error() {
  setup_tracing();

  struct TypedErrorProbe {
    saw_typed: ContextData<Vec<(String, StepPhase, bool)>>,
  }
  impl PipelineObserver for TypedErrorProbe {
    fn on_event(&self, _event: &TraceEvent) {}
    fn on_handler_error(&self, _run_id: u64, step: &str, phase: StepPhase, error: &(dyn std::error::Error + 'static)) {
      // The buffered event only has a String; here the concrete type is reachable.
      let is_expected_variant = matches!(
        error.downcast_ref::<TestError>(),
        Some(TestError::Handler(msg)) if msg == "typed"
      );
      self.saw_typed.write().push((step.to_string(), phase, is_expected_variant));
    }
  }

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["ok_step", "bad_step"]);
  p.on_root("ok_step", |_ctx| async { Ok(PipelineControl::Continue) });
  p.on_root("bad_step", |_ctx| async { Err(TestError::Handler("typed".into())) });

  let saw = ContextData::new(Vec::new());
  p.set_observer(Arc::new(TypedErrorProbe { saw_typed: saw.clone() }));

  let _ = p.run(ContextData::new(TestContext::default())).await;
  assert_eq!(saw.read().as_slice(), &[("bad_step".to_string(), StepPhase::On, true)]);
}

#[tokio::test]
async fn clear_observer_detaches() {
  setup_tracing();
  let (p, trace) = traced_pipeline();
  p.clear_observer();
  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert!(trace.events().is_empty());
}

#[tokio::test]
async fn labeled_skip_appears_in_trace_events_and_display() {
  setup_tracing();
  let (mut p, trace) = traced_pipeline();
  p.skip_if_labeled("two", "fresh deploy, nothing to drain", |_ctx| true);
  p.run(ContextData::new(TestContext::default())).await.unwrap();

  let skipped: Vec<TraceEvent> = trace
    .events()
    .into_iter()
    .filter(|e| matches!(e.kind, TraceEventKind::StepSkipped { .. }))
    .collect();
  assert_eq!(skipped.len(), 1);
  match &skipped[0].kind {
    TraceEventKind::StepSkipped { step, reason, .. } => {
      assert_eq!(step, "two");
      assert_eq!(
        reason,
        &SkipReason::SkipCondition {
          label: Some("fresh deploy, nothing to drain".to_string())
        }
      );
    }
    other => panic!("unexpected kind: {:?}", other),
  }
  // Display renders the label itself, so previews and logs read as documentation.
  assert!(skipped[0].to_string().contains("fresh deploy, nothing to drain"));
}

#[tokio::test]
async fn composite_observer_fans_out_to_all_observers() {
  setup_tracing();
  let (p, _own) = traced_pipeline();
  p.clear_observer();

  let first = TraceCollector::new();
  let second = TraceCollector::new();
  let mut composite = CompositeObserver::new();
  composite.push(Arc::new(first.clone()));
  composite.push(Arc::new(second.clone()));
  p.set_observer(Arc::new(composite));

  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert_eq!(first.completed_steps(), vec!["one", "two", "three"]);
  assert_eq!(second.completed_steps(), vec!["one", "two", "three"]);

  // on_handler_error fans out too.
  let (p2, _own2) = traced_pipeline();
  p2.clear_observer();
  let mut p2 = p2;
  p2.fail_at("two", || TestError::Handler("fanout".into()));
  let a = TraceCollector::new();
  let b = TraceCollector::new();
  p2.set_observer(Arc::new(CompositeObserver::with(vec![
    Arc::new(a.clone()),
    Arc::new(b.clone()),
  ])));
  let _ = p2.run(ContextData::new(TestContext::default())).await;
  for t in [&a, &b] {
    assert!(matches!(t.last_outcome(), Some(RunOutcome::Errored { .. })));
  }
}

#[tokio::test]
async fn run_with_outcome_attributes_the_failing_step() {
  setup_tracing();
  let (mut p, _trace) = traced_pipeline();
  p.fail_at("two", || TestError::Handler("kaput".into()));

  let (result, outcome) = p.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert_eq!(result.unwrap_err(), TestError::Handler("kaput".into()));
  match outcome {
    RunOutcome::Errored { step, message } => {
      assert_eq!(step, "two");
      assert!(message.contains("kaput"));
    }
    other => panic!("expected Errored, got {:?}", other),
  }

  // Ok paths report their outcome too.
  let (p_ok, _t) = traced_pipeline();
  let (result, outcome) = p_ok.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert!(result.is_ok());
  assert_eq!(outcome, RunOutcome::Completed);

  // A finish failure on an Ok run is attributed to "on_finish".
  let (mut p_fin, _t2) = traced_pipeline();
  p_fin.on_finish(|_ctx, _outcome| async { Err(TestError::Other("cleanup".into())) });
  let (result, outcome) = p_fin.run_with_outcome(ContextData::new(TestContext::default())).await;
  assert!(result.is_err());
  assert!(matches!(outcome, RunOutcome::Errored { ref step, .. } if step == "on_finish"));
}

// --- Per-run observers ---

/// The hole this closes: a pipeline-attached observer collects every concurrent run, and
/// you cannot filter to your own because the run id is allocated inside `run`.
#[tokio::test]
async fn a_scoped_observer_collects_only_its_own_call() {
  setup_tracing();
  let (p, _unused) = traced_pipeline();
  p.clear_observer();
  let p = Arc::new(p);

  let mine = TraceCollector::new();
  let theirs = TraceCollector::new();

  let (a, b) = (p.clone(), p.clone());
  let (mine_arc, theirs_arc) = (Arc::new(mine.clone()), Arc::new(theirs.clone()));

  let first = tokio::spawn(async move {
    a.run_with_observer(ContextData::new(TestContext::default()), mine_arc).await
  });
  let second = tokio::spawn(async move {
    b.run_with_observer(ContextData::new(TestContext::default()), theirs_arc).await
  });
  first.await.unwrap().0.unwrap();
  second.await.unwrap().0.unwrap();

  assert_eq!(mine.run_count(), 1, "each collector sees exactly one run");
  assert_eq!(theirs.run_count(), 1);
  assert_eq!(mine.run_ids().len(), 1);
  assert_ne!(mine.run_ids()[0], theirs.run_ids()[0], "and they are different runs");
}

#[tokio::test]
async fn a_scoped_observer_does_not_displace_the_attached_one() {
  setup_tracing();
  let (p, attached) = traced_pipeline();

  let scoped = TraceCollector::new();
  p.run_with_observer(ContextData::new(TestContext::default()), Arc::new(scoped.clone()))
    .await
    .0
    .unwrap();

  assert_steps_completed(&attached, &["one", "two", "three"]);
  assert_steps_completed(&scoped, &["one", "two", "three"]);
}

/// A scoped observer is inherited by runs started from inside a handler, so one collector
/// sees the whole call tree. A pipeline-attached observer deliberately is not.
#[tokio::test]
async fn a_scoped_observer_reaches_nested_runs_and_an_attached_one_does_not() {
  setup_tracing();
  let mut branch: Pipeline<TestContext, TestError> = Pipeline::new(["branch_work"]);
  branch.on_root("branch_work", |_ctx| async { Ok(PipelineControl::Continue) });
  let branch = Arc::new(branch);

  let mut parent: Pipeline<TestContext, TestError> = Pipeline::new(["fan"]);
  let branch_for_step = branch.clone();
  parent.on_root("fan", move |_ctx| {
    let branch = branch_for_step.clone();
    async move {
      let results = orka::FanOut::new(branch)
        .run(vec![TestContext::default(), TestContext::default()])
        .await;
      assert_eq!(results.succeeded(), 2);
      Ok(PipelineControl::Continue)
    }
  });

  // Attached: sees only this pipeline's own run.
  let attached = TraceCollector::new();
  parent.set_tracer(attached.clone());
  parent.run(ContextData::new(TestContext::default())).await.unwrap();
  assert!(
    !attached.completed_steps().iter().any(|s| s == "branch_work"),
    "an attached observer stays bound to its own pipeline: {:?}",
    attached.completed_steps()
  );

  // Scoped: inherited by the branches, so the tree lands in one collector.
  let scoped = TraceCollector::new();
  parent
    .run_with_observer(ContextData::new(TestContext::default()), Arc::new(scoped.clone()))
    .await
    .0
    .unwrap();

  let branch_runs = scoped
    .completed_steps()
    .iter()
    .filter(|s| *s == "branch_work")
    .count();
  assert_eq!(branch_runs, 2, "both fan-out branches reported into the scoped collector");
  assert!(scoped.completed_steps().iter().any(|s| s == "fan"));
  assert_eq!(scoped.run_ids().len(), 3, "the parent run plus one per branch");
}
