//! Tests for reading declared resources without panicking (`ContextData::require`) and
//! for taking a resource out of the bag on loan (`RunResources::take_guard`).

mod common;

use common::{setup_tracing, TestError};
use orka::{ContextData, OrkaError, Pipeline, PipelineControl, RunOutcome};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, Default)]
struct BuildCtx {
  produce_spec: bool,
  app_spec: Option<String>,
  saw_spec: Option<String>,
}

/// The pair a real pipeline declares with `produces` / `consumed_by`, exercised end to end.
fn spec_pipeline() -> Pipeline<BuildCtx, TestError> {
  let mut p: Pipeline<BuildCtx, TestError> = Pipeline::new(["load_spec", "runtime_labels"]);

  p.on_root("load_spec", |ctx: ContextData<BuildCtx>| async move {
    if ctx.with_ref(|c| c.produce_spec) {
      ctx.with_mut(|c| c.app_spec = Some("spec-v1".to_string()));
    }
    Ok(PipelineControl::Continue)
  })
  .on_root("runtime_labels", |ctx: ContextData<BuildCtx>| async move {
    // The `.expect("app_spec set by load-spec step")` this replaces.
    let spec = ctx.require("app spec", |c| c.app_spec.clone())?;
    ctx.with_mut(|c| c.saw_spec = Some(spec));
    Ok(PipelineControl::Continue)
  })
  .produces("load_spec", "app spec")
  .consumed_by("app spec", ["runtime_labels"]);

  p
}

#[tokio::test]
async fn require_reads_a_produced_resource() {
  setup_tracing();
  let p = spec_pipeline();
  assert!(p.validate().is_ok());

  let ctx = ContextData::new(BuildCtx {
    produce_spec: true,
    ..BuildCtx::default()
  });
  p.run(ctx.clone()).await.unwrap();
  assert_eq!(ctx.with_ref(|c| c.saw_spec.clone()), Some("spec-v1".to_string()));
}

/// The point of `require`: a missing resource is a handled error, so the run still reaches
/// its finish ring and releases its resource bag. A panicking `.expect()` would unwind past
/// both, and would leave what does drop dropping front to back rather than in reverse.
#[tokio::test]
async fn a_missing_resource_is_an_error_that_still_runs_cleanup() {
  setup_tracing();
  let mut p = spec_pipeline();

  let finished = Arc::new(AtomicUsize::new(0));
  let released = Arc::new(AtomicUsize::new(0));

  struct Marker(Arc<AtomicUsize>);
  impl Drop for Marker {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  let released_in_step = released.clone();
  p.replace_on_root("load_spec", move |ctx: ContextData<BuildCtx>| {
    let released = released_in_step.clone();
    async move {
      // Acquire something the run must release, then fail to produce the spec.
      ctx.resources().put(Marker(released.clone()));
      Ok(PipelineControl::Continue)
    }
  });

  let finished_in_ring = finished.clone();
  p.on_finish(move |_ctx, outcome| {
    let finished = finished_in_ring.clone();
    async move {
      assert!(matches!(outcome, RunOutcome::Errored { .. }));
      finished.fetch_add(1, Ordering::SeqCst);
      Ok(())
    }
  });

  let ctx = ContextData::new(BuildCtx::default()); // produce_spec is false
  let (result, outcome) = p.run_with_outcome(ctx).await;

  match result.unwrap_err() {
    TestError::Orka(s) => {
      assert!(s.contains("ResourceMissing"), "got: {}", s);
      assert!(s.contains("app spec"), "the error names the resource: {}", s);
    }
    other => panic!("expected a handled framework error, got {:?}", other),
  }
  // The step comes from the engine, since a context does not know who is reading it.
  assert!(matches!(outcome, RunOutcome::Errored { ref step, .. } if step == "runtime_labels"));

  assert_eq!(finished.load(Ordering::SeqCst), 1, "the finish ring still ran");
  assert_eq!(released.load(Ordering::SeqCst), 1, "and the resource bag still released");
}

#[tokio::test]
async fn require_can_be_used_outside_a_pipeline() {
  setup_tracing();
  let ctx = ContextData::new(BuildCtx::default());
  let err = ctx.require("app spec", |c| c.app_spec.clone()).unwrap_err();
  assert!(matches!(err, OrkaError::ResourceMissing { ref resource } if resource == "app spec"));
}

// --- take / take_guard ---

/// Stands in for a stream sender: operated on across awaits, and needing an explicit
/// shutdown that must not be skipped.
#[derive(Debug)]
struct StreamSender {
  sent: usize,
  log: Arc<Mutex<Vec<String>>>,
}

impl StreamSender {
  async fn send(&mut self, chunk: &str) {
    tokio::time::sleep(Duration::from_millis(1)).await;
    self.sent += 1;
    self.log.lock().unwrap().push(format!("sent {}", chunk));
  }
}

impl Drop for StreamSender {
  fn drop(&mut self) {
    self.log.lock().unwrap().push(format!("dropped after {} chunk(s)", self.sent));
  }
}

#[tokio::test]
async fn take_guard_lends_a_resource_across_awaits_and_returns_it() {
  setup_tracing();
  let log = Arc::new(Mutex::new(Vec::new()));

  let mut p: Pipeline<BuildCtx, TestError> = Pipeline::new(["open", "upload"]);
  let open_log = log.clone();
  p.on_root("open", move |ctx: ContextData<BuildCtx>| {
    let log = open_log.clone();
    async move {
      ctx.resources().put(StreamSender { sent: 0, log: log.clone() });
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("upload", |ctx: ContextData<BuildCtx>| async move {
    let mut sender = ctx
      .resources()
      .take_guard::<StreamSender>()
      .expect("stashed by the open step");

    // The guard is not a lock guard, so it may be held across suspension points.
    for chunk in ["a", "b", "c"] {
      sender.send(chunk).await;
    }
    assert_eq!(sender.sent, 3);
    Ok(PipelineControl::Continue)
    // dropping the guard here returns the sender to the bag
  });

  let ctx = ContextData::new(BuildCtx::default());
  p.run(ctx.clone()).await.unwrap();

  let entries = log.lock().unwrap().clone();
  assert_eq!(
    entries,
    vec!["sent a", "sent b", "sent c", "dropped after 3 chunk(s)"],
    "the sender was released once, by the bag, after its work"
  );
  assert!(ctx.resources().is_empty(), "and the bag was drained by the run");
}

/// The property raw `take` would lose: a handler abandoned mid-await still returns the
/// resource to the bag, so it is released at the run's defined point rather than dropping
/// inside a cancelled future.
#[tokio::test]
async fn a_lent_resource_returns_to_the_bag_when_its_handler_is_cancelled() {
  setup_tracing();
  let log = Arc::new(Mutex::new(Vec::new()));
  let finish_saw = Arc::new(AtomicUsize::new(0));

  let mut p: Pipeline<BuildCtx, TestError> = Pipeline::new(["open", "upload"]);
  let open_log = log.clone();
  p.on_root("open", move |ctx: ContextData<BuildCtx>| {
    let log = open_log.clone();
    async move {
      ctx.resources().put(StreamSender { sent: 0, log: log.clone() });
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("upload", |ctx: ContextData<BuildCtx>| async move {
    let mut sender = ctx.resources().take_guard::<StreamSender>().expect("stashed");
    // A budget that expires mid-upload: `timed` drops this future, and with it the guard.
    orka::timed("upload", Duration::from_millis(5), async move {
      for chunk in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        sender.send(chunk).await;
      }
    })
    .await?;
    Ok(PipelineControl::Continue)
  });

  // The finish ring runs before the bag is released, so the sender is still there to see.
  let finish_saw_in_ring = finish_saw.clone();
  p.on_finish(move |ctx: ContextData<BuildCtx>, _outcome| {
    let seen = finish_saw_in_ring.clone();
    async move {
      let held = ctx.resources().with(|s: &StreamSender| s.sent);
      if held.is_some() {
        seen.fetch_add(1, Ordering::SeqCst);
      }
      Ok(())
    }
  });

  let ctx = ContextData::new(BuildCtx::default());
  let err = p.run(ctx.clone()).await.unwrap_err();
  assert!(matches!(&err, TestError::Orka(s) if s.contains("StepTimedOut")), "got {:?}", err);

  assert_eq!(
    finish_saw.load(Ordering::SeqCst),
    1,
    "the cancelled handler returned the sender, so on_finish could still reach it"
  );

  let entries = log.lock().unwrap().clone();
  assert!(
    entries.last().unwrap().starts_with("dropped after"),
    "and it was released by the bag, not inside the dropped future: {:?}",
    entries
  );
  assert!(ctx.resources().is_empty());
}

#[tokio::test]
async fn take_removes_permanently_and_keep_opts_out_of_the_return() {
  setup_tracing();
  let ctx = ContextData::new(BuildCtx::default());

  ctx.resources().put(7_u32).put(9_u32);
  assert_eq!(ctx.resources().take::<u32>(), Some(9), "most recently stashed first");
  assert_eq!(ctx.resources().len(), 1);
  assert_eq!(ctx.resources().take::<u32>(), Some(7));
  assert!(ctx.resources().is_empty());
  assert_eq!(ctx.resources().take::<u32>(), None);

  ctx.resources().put(String::from("held"));
  let kept = ctx
    .resources()
    .take_guard::<String>()
    .expect("present")
    .keep();
  assert_eq!(kept, "held");
  assert!(ctx.resources().is_empty(), "keep() opts out of the put-back");
}
