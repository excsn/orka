//! Tests for the run-scoped resource bag: reverse drop order, release after the finish
//! ring, typed borrow, and the partial-runner and orphan-handle fallbacks.

mod common;

use common::{setup_tracing, TestContext, TestError};
use orka::test_util::PipelineTestExt;
use orka::{ContextData, Pipeline, PipelineControl, TraceCollector, TraceEventKind};
use std::sync::{Arc, Mutex};

/// Records the order in which its markers are dropped, so tests can assert RAII ordering
/// rather than just "it happened".
#[derive(Clone, Default)]
struct DropLog(Arc<Mutex<Vec<&'static str>>>);

impl DropLog {
  fn marker(&self, name: &'static str) -> DropMarker {
    DropMarker {
      name,
      log: self.clone(),
    }
  }

  fn note(&self, entry: &'static str) {
    self.0.lock().unwrap().push(entry);
  }

  fn entries(&self) -> Vec<&'static str> {
    self.0.lock().unwrap().clone()
  }
}

struct DropMarker {
  name: &'static str,
  log: DropLog,
}

impl Drop for DropMarker {
  fn drop(&mut self) {
    self.log.note(self.name);
  }
}

#[tokio::test]
async fn resources_drop_in_reverse_order_after_finish_handlers() {
  setup_tracing();
  let log = DropLog::default();

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["acquire", "work"]);
  let acquire_log = log.clone();
  p.on_root("acquire", move |ctx| {
    let log = acquire_log.clone();
    async move {
      // The shape this replaces: Option<OwnedMutexGuard> and Option<TempDir> fields on
      // the context, taken and dropped by hand in a finish handler.
      ctx.resources().put(log.marker("lock guard")).put(log.marker("temp dir"));
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("work", |_ctx| async { Ok(PipelineControl::Continue) });

  let finish_log = log.clone();
  p.on_finish(move |_ctx, _outcome| {
    let log = finish_log.clone();
    async move {
      log.note("finish handler");
      Ok(())
    }
  });

  let ctx = ContextData::new(TestContext::default());
  p.run(ctx.clone()).await.unwrap();

  // The finish handler runs while both resources are still alive, then they release
  // newest-first.
  assert_eq!(log.entries(), vec!["finish handler", "temp dir", "lock guard"]);
  assert!(ctx.resources().is_empty());
}

#[tokio::test]
async fn resources_release_even_when_the_run_fails() {
  setup_tracing();
  let log = DropLog::default();

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["acquire", "explode"]);
  let acquire_log = log.clone();
  p.on_root("acquire", move |ctx| {
    let log = acquire_log.clone();
    async move {
      ctx.resources().put(log.marker("lock guard"));
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("explode", |_ctx| async { Ok(PipelineControl::Continue) });
  p.fail_at("explode", || TestError::Handler("boom".into()));

  let ctx = ContextData::new(TestContext::default());
  let err = p.run(ctx.clone()).await.unwrap_err();

  assert_eq!(err, TestError::Handler("boom".into()));
  assert_eq!(log.entries(), vec!["lock guard"], "release is unconditional, like Drop");
  assert!(ctx.resources().is_empty());
}

#[tokio::test]
async fn with_borrows_the_most_recently_stashed_value_of_a_type() {
  setup_tracing();
  let ctx = ContextData::new(TestContext::default());

  // Nothing of that type held yet.
  assert_eq!(ctx.resources().with(|s: &String| s.clone()), None);

  ctx
    .resources()
    .put("first".to_string())
    .put("second".to_string())
    .put(7_u32);

  // This is how a TempDir stays reachable for its path without duplicating it as data.
  assert_eq!(
    ctx.resources().with(|s: &String| s.clone()),
    Some("second".to_string())
  );
  assert_eq!(ctx.resources().with(|n: &u32| *n), Some(7));
  assert_eq!(ctx.resources().with(|b: &bool| *b), None);
  assert_eq!(ctx.resources().len(), 3);
}

#[tokio::test]
async fn partial_runners_leave_the_bag_alone() {
  setup_tracing();
  let log = DropLog::default();

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["acquire", "work"]);
  let acquire_log = log.clone();
  p.on_root("acquire", move |ctx| {
    let log = acquire_log.clone();
    async move {
      ctx.resources().put(log.marker("temp dir"));
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("work", |_ctx| async { Ok(PipelineControl::Continue) });

  // Same rule as the finish ring: step isolation is an inspection tool, so what the step
  // stashed is still there to look at afterwards.
  let ctx = ContextData::new(TestContext::default());
  p.run_step("acquire", ctx.clone()).await.unwrap();
  assert_eq!(ctx.resources().len(), 1);
  assert!(log.entries().is_empty());

  // A subsequent full run releases it.
  p.run(ctx.clone()).await.unwrap();
  assert_eq!(log.entries(), vec!["temp dir", "temp dir"]);
  assert!(ctx.resources().is_empty());
}

#[tokio::test]
async fn anything_still_held_drops_with_the_last_context_handle() {
  setup_tracing();
  let log = DropLog::default();

  let ctx = ContextData::new(TestContext::default());
  ctx.resources().put(log.marker("orphan"));
  let second_handle = ctx.clone();

  drop(ctx);
  assert!(log.entries().is_empty(), "another handle is still alive");

  drop(second_handle);
  assert_eq!(log.entries(), vec!["orphan"], "no leak without a run");
}

#[tokio::test]
async fn release_is_traced_only_when_something_was_held() {
  setup_tracing();

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |_ctx| async { Ok(PipelineControl::Continue) });
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  // Empty bag: no event, so traces of pipelines that hold nothing are unchanged.
  p.run(ContextData::new(TestContext::default())).await.unwrap();
  assert!(!trace
    .events()
    .iter()
    .any(|e| matches!(e.kind, TraceEventKind::ResourcesReleased { .. })));

  // Something held: the release is observable, which is the point when the whole
  // mechanism is otherwise invisible.
  trace.clear();
  p.replace_on_root("work", |ctx| async move {
    ctx.resources().put(String::from("a")).put(String::from("b"));
    Ok(PipelineControl::Continue)
  });
  p.run(ContextData::new(TestContext::default())).await.unwrap();

  let released: Vec<usize> = trace
    .events()
    .into_iter()
    .filter_map(|e| match e.kind {
      TraceEventKind::ResourcesReleased { count } => Some(count),
      _ => None,
    })
    .collect();
  assert_eq!(released, vec![2]);

  // It lands after the finish ring and before the run's final event.
  let kinds: Vec<String> = trace.events().iter().map(|e| e.kind.to_string()).collect();
  let release_at = kinds.iter().position(|k| k.contains("released")).unwrap();
  let finished_at = kinds.iter().position(|k| k.starts_with("run finished")).unwrap();
  assert!(release_at < finished_at);
}
