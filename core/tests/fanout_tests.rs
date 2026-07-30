//! Tests for all-of-N fan-out: bounded concurrency, input ordering, typed per-item
//! errors, the policy verdicts, and FailFast draining rather than cancelling.

mod common;

use common::{setup_tracing, TestError};
use orka::test_util::PipelineTestExt;
use orka::{
  CancelToken, ContextData, FanOut, FanOutItemOutcome, FanOutPolicy, Pipeline, PipelineControl, PipelineResult,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq)]
struct Item {
  id: usize,
  /// Milliseconds this item's work takes, so tests can force out-of-order completion.
  work_ms: u64,
  processed: bool,
}

/// Tracks how many branches are in flight at once, and the high-water mark.
#[derive(Clone, Default)]
struct ConcurrencyProbe {
  live: Arc<AtomicUsize>,
  peak: Arc<AtomicUsize>,
}

impl ConcurrencyProbe {
  fn enter(&self) {
    let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
    self.peak.fetch_max(now, Ordering::SeqCst);
  }

  fn exit(&self) {
    self.live.fetch_sub(1, Ordering::SeqCst);
  }

  fn peak(&self) -> usize {
    self.peak.load(Ordering::SeqCst)
  }
}

/// A branch pipeline that sleeps for the item's `work_ms`, tracking concurrency.
fn probed_pipeline(probe: ConcurrencyProbe) -> Arc<Pipeline<Item, TestError>> {
  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", move |ctx: ContextData<Item>| {
    let probe = probe.clone();
    async move {
      probe.enter();
      let work_ms = ctx.with_ref(|i| i.work_ms);
      tokio::time::sleep(Duration::from_millis(work_ms)).await;
      ctx.with_mut(|i| i.processed = true);
      probe.exit();
      Ok(PipelineControl::Continue)
    }
  });
  Arc::new(p)
}

fn items(count: usize, work_ms: u64) -> Vec<Item> {
  (0..count)
    .map(|id| Item {
      id,
      work_ms,
      processed: false,
    })
    .collect()
}

#[tokio::test]
async fn respects_the_concurrency_limit() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();
  let fan_out = FanOut::new(probed_pipeline(probe.clone())).max_concurrent(3);

  let results = fan_out.run(items(9, 5)).await;

  assert_eq!(results.len(), 9);
  assert_eq!(results.succeeded(), 9);
  assert!(
    probe.peak() <= 3,
    "at most 3 branches should ever be in flight, saw {}",
    probe.peak()
  );
  assert!(probe.peak() > 1, "and it should actually overlap, saw {}", probe.peak());
}

#[tokio::test]
async fn a_limit_of_one_is_fully_sequential() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();
  let results = FanOut::new(probed_pipeline(probe.clone()))
    .max_concurrent(1)
    .run(items(4, 2))
    .await;

  assert_eq!(results.succeeded(), 4);
  assert_eq!(probe.peak(), 1);
}

/// The fill-then-poll regression guard: with a single-pass poll, branches promoted into a
/// freed slot would never receive a first poll and this would hang rather than fail.
#[tokio::test]
async fn more_items_than_the_limit_still_all_complete() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();
  let results = FanOut::new(probed_pipeline(probe))
    .max_concurrent(2)
    .run(items(10, 1))
    .await;

  assert_eq!(results.succeeded(), 10);
  for (i, item) in results.items().iter().enumerate() {
    assert_eq!(item.index, i);
    assert!(item.context.with_ref(|it| it.processed));
  }
}

#[tokio::test]
async fn results_keep_input_order_even_when_completion_order_is_reversed() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();
  // Descending durations: the last item finishes first.
  let ladder: Vec<Item> = (0..5)
    .map(|id| Item {
      id,
      work_ms: (5 - id as u64) * 6,
      processed: false,
    })
    .collect();

  let results = FanOut::new(probed_pipeline(probe)).run(ladder).await;

  let ids: Vec<usize> = results.items().iter().map(|i| i.context.with_ref(|it| it.id)).collect();
  assert_eq!(ids, vec![0, 1, 2, 3, 4], "input order, not completion order");
  assert_eq!(results.cloned_oks().len(), 5);
}

#[tokio::test]
async fn per_item_errors_stay_typed_and_keep_their_index() {
  setup_tracing();
  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |ctx: ContextData<Item>| async move {
    let id = ctx.with_ref(|i| i.id);
    if id % 2 == 1 {
      return Err(TestError::Handler(format!("item {} failed", id)));
    }
    ctx.with_mut(|i| i.processed = true);
    Ok(PipelineControl::Continue)
  });

  let results = FanOut::new(Arc::new(p)).run(items(5, 0)).await;

  assert_eq!(results.succeeded(), 3);
  assert_eq!(results.failed(), 2);

  let errors: Vec<(usize, String)> = results.errors().map(|(i, e)| (i, e.to_string())).collect();
  assert_eq!(errors.len(), 2);
  assert_eq!(errors[0].0, 1, "the failing item's own index, not a position in a filtered list");
  assert_eq!(errors[1].0, 3);
  assert!(errors[0].1.contains("item 1 failed"));

  // And the typed error survives, rather than being flattened into a string.
  match &results.items()[1].outcome {
    FanOutItemOutcome::Failed(TestError::Handler(msg)) => assert_eq!(msg, "item 1 failed"),
    other => panic!("expected a typed Handler error, got {:?}", other),
  }
}

#[tokio::test]
async fn fail_fast_drains_in_flight_branches_instead_of_cancelling_them() {
  setup_tracing();
  let finished = Arc::new(Mutex::new(Vec::<usize>::new()));

  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |ctx: ContextData<Item>| async move {
    let (id, work_ms) = ctx.with_ref(|i| (i.id, i.work_ms));
    tokio::time::sleep(Duration::from_millis(work_ms)).await;
    if id == 0 {
      return Err(TestError::Handler("first item fails immediately".into()));
    }
    Ok(PipelineControl::Continue)
  });

  // The finish ring is exactly what cancellation would skip, so record it per branch.
  let recorder = finished.clone();
  p.on_finish(move |ctx: ContextData<Item>, _outcome| {
    let recorder = recorder.clone();
    async move {
      let id = ctx.with_ref(|i| i.id);
      recorder.lock().unwrap().push(id);
      Ok(())
    }
  });

  // Item 0 fails at once; items 1 and 2 are already in flight and take longer.
  let ladder = vec![
    Item { id: 0, work_ms: 0, processed: false },
    Item { id: 1, work_ms: 25, processed: false },
    Item { id: 2, work_ms: 25, processed: false },
    Item { id: 3, work_ms: 0, processed: false },
    Item { id: 4, work_ms: 0, processed: false },
  ];

  let results = FanOut::new(Arc::new(p))
    .policy(FanOutPolicy::FailFast)
    .max_concurrent(3)
    .run(ladder)
    .await;

  assert!(!results.satisfied(), "a failure means FailFast is unmet");
  assert_eq!(results.failed(), 1);

  // Items 3 and 4 never started, so they ran no code at all.
  assert_eq!(results.not_started(), 2);
  assert!(results.items()[3].outcome.is_not_started());
  assert!(results.items()[4].outcome.is_not_started());

  // The decisive assertion: the two branches already in flight were drained, not dropped,
  // so their finish handlers ran. A cancelling implementation would omit 1 and 2 here.
  let mut ran_finish = finished.lock().unwrap().clone();
  ran_finish.sort_unstable();
  assert_eq!(ran_finish, vec![0, 1, 2]);
}

#[tokio::test]
async fn policy_verdicts() {
  setup_tracing();
  let build = || {
    let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
    p.on_root("work", |ctx: ContextData<Item>| async move {
      let id = ctx.with_ref(|i| i.id);
      if id >= 3 {
        return Err(TestError::Handler(format!("item {} failed", id)));
      }
      Ok(PipelineControl::Continue)
    });
    Arc::new(p)
  };

  // 3 of 5 succeed: exactly the case the hand-rolled version cannot report.
  let collect_all = FanOut::new(build()).run(items(5, 0)).await;
  assert!(collect_all.satisfied(), "CollectAll never fails on branch errors");
  assert_eq!((collect_all.succeeded(), collect_all.failed()), (3, 2));

  let require_all = FanOut::new(build()).policy(FanOutPolicy::RequireAll).run(items(5, 0)).await;
  assert!(!require_all.satisfied());

  let at_least_3 = FanOut::new(build())
    .policy(FanOutPolicy::RequireAtLeast(3))
    .run(items(5, 0))
    .await;
  assert!(at_least_3.satisfied());

  let at_least_4 = FanOut::new(build())
    .policy(FanOutPolicy::RequireAtLeast(4))
    .run(items(5, 0))
    .await;
  assert!(!at_least_4.satisfied());

  // The escape hatch: satisfied only if item 0 specifically succeeded.
  let custom = FanOut::new(build())
    .custom_policy(|r| r.items()[0].outcome.is_success())
    .run(items(5, 0))
    .await;
  assert!(custom.satisfied());
}

#[tokio::test]
async fn into_control_returns_the_first_typed_error_then_falls_back_to_policy_unmet() {
  setup_tracing();
  let mut failing: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  failing.on_root("work", |_ctx| async { Ok(PipelineControl::Continue) });
  failing.fail_at("work", || TestError::Handler("boom".into()));

  let results = FanOut::new(Arc::new(failing))
    .policy(FanOutPolicy::RequireAll)
    .run(items(2, 0))
    .await;
  assert_eq!(
    results.into_control().unwrap_err(),
    TestError::Handler("boom".into()),
    "a real branch error propagates rather than being replaced"
  );

  // Unmet with no branch error at all: every branch merely stopped.
  let mut stopping: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  stopping.on_root("work", |_ctx| async { Ok(PipelineControl::Stop) });
  let stopped = FanOut::new(Arc::new(stopping))
    .custom_policy(|_| false)
    .run(items(2, 0))
    .await;
  assert_eq!(stopped.succeeded(), 2, "a stopped branch ran without error");
  assert_eq!(stopped.stopped(), 2);
  match stopped.into_control().unwrap_err() {
    TestError::Orka(msg) => assert!(msg.contains("FanOutPolicyUnmet"), "got: {}", msg),
    other => panic!("expected the synthesized policy error, got {:?}", other),
  }
}

#[tokio::test]
async fn each_branch_is_a_full_run_with_its_own_finish_ring_and_resources() {
  setup_tracing();
  let released = Arc::new(AtomicUsize::new(0));
  let finished = Arc::new(AtomicUsize::new(0));

  struct Marker(Arc<AtomicUsize>);
  impl Drop for Marker {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  let released_in_handler = released.clone();
  p.on_root("work", move |ctx: ContextData<Item>| {
    let released = released_in_handler.clone();
    async move {
      ctx.resources().put(Marker(released.clone()));
      Ok(PipelineControl::Continue)
    }
  });
  let finished_in_ring = finished.clone();
  p.on_finish(move |_ctx, _outcome| {
    let finished = finished_in_ring.clone();
    async move {
      finished.fetch_add(1, Ordering::SeqCst);
      Ok(())
    }
  });

  let results = FanOut::new(Arc::new(p)).run(items(4, 0)).await;

  assert_eq!(results.succeeded(), 4);
  assert_eq!(finished.load(Ordering::SeqCst), 4, "every branch fired its own finish ring");
  assert_eq!(released.load(Ordering::SeqCst), 4, "and released its own resource bag");
}

#[tokio::test]
async fn empty_and_single_item_collections() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();
  let fan_out = FanOut::new(probed_pipeline(probe));

  let empty = fan_out.run(Vec::new()).await;
  assert!(empty.is_empty());
  assert!(empty.satisfied(), "CollectAll over nothing is satisfied");

  let one = fan_out.run(items(1, 0)).await;
  assert_eq!(one.len(), 1);
  assert_eq!(one.succeeded(), 1);
  assert!(matches!(
    one.items()[0].outcome,
    FanOutItemOutcome::Completed(PipelineResult::Completed)
  ));
}

#[tokio::test]
async fn results_display_summarises_the_run() {
  setup_tracing();
  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |ctx: ContextData<Item>| async move {
    if ctx.with_ref(|i| i.id) == 0 {
      return Err(TestError::Handler("nope".into()));
    }
    Ok(PipelineControl::Continue)
  });

  let results = FanOut::new(Arc::new(p)).policy(FanOutPolicy::RequireAll).run(items(3, 0)).await;
  let rendered = results.to_string();
  assert!(rendered.contains("3 item(s)"), "got: {}", rendered);
  assert!(rendered.contains("2 succeeded"), "got: {}", rendered);
  assert!(rendered.contains("1 failed"), "got: {}", rendered);
  assert!(rendered.contains("RequireAll: unmet"), "got: {}", rendered);
}

#[test]
#[should_panic(expected = "max_concurrent must be at least 1")]
fn zero_concurrency_is_a_setup_error() {
  let p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  let _ = FanOut::new(Arc::new(p)).max_concurrent(0);
}

// --- Spawned branches: opting into real parallelism ---

/// A consumer-written spawner, proving the trait is implementable outside orka (and
/// counting spawns, which the shipped `TokioSpawner` does not).
struct CountingSpawner {
  spawns: Arc<AtomicUsize>,
}

impl orka::TaskSpawner for CountingSpawner {
  fn spawn(&self, task: orka::SpawnedTask) -> orka::SpawnHandle {
    self.spawns.fetch_add(1, Ordering::SeqCst);
    let handle = tokio::spawn(task);
    // Resolve on panic or abort too, so orka reports a lost branch rather than hanging.
    Box::pin(async move {
      let _ = handle.await;
    })
  }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawner_runs_branches_as_tasks_while_keeping_order_and_results() {
  setup_tracing();
  let spawns = Arc::new(AtomicUsize::new(0));
  let probe = ConcurrencyProbe::default();

  let results = FanOut::new(probed_pipeline(probe.clone()))
    .spawner(Arc::new(CountingSpawner { spawns: spawns.clone() }))
    .run(items(6, 5))
    .await;

  assert_eq!(results.succeeded(), 6);
  assert_eq!(spawns.load(Ordering::SeqCst), 6, "one task per branch");

  let ids: Vec<usize> = results.items().iter().map(|i| i.context.with_ref(|it| it.id)).collect();
  assert_eq!(ids, vec![0, 1, 2, 3, 4, 5], "still input order");
  for item in results.items() {
    assert!(item.context.with_ref(|it| it.processed), "each branch's writes are visible");
  }
}

/// Spawning happens on a branch's first poll, so the joiner's concurrency cap still
/// governs how many tasks exist at once even though it knows nothing about spawning.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_concurrent_still_bounds_spawned_branches() {
  setup_tracing();
  let spawns = Arc::new(AtomicUsize::new(0));
  let probe = ConcurrencyProbe::default();

  let results = FanOut::new(probed_pipeline(probe.clone()))
    .spawner(Arc::new(CountingSpawner { spawns: spawns.clone() }))
    .max_concurrent(2)
    .run(items(8, 10))
    .await;

  assert_eq!(results.succeeded(), 8);
  assert_eq!(spawns.load(Ordering::SeqCst), 8);
  assert!(
    probe.peak() <= 2,
    "at most 2 branches in flight despite spawning, saw {}",
    probe.peak()
  );
}

/// The behaviour that only exists in spawned mode: the runtime contains the panic, orka
/// turns the lost branch into a typed failure, and every sibling still completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_branch_is_contained_and_reported_as_lost() {
  setup_tracing();
  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |ctx: ContextData<Item>| async move {
    if ctx.with_ref(|i| i.id) == 1 {
      panic!("branch 1 panics");
    }
    ctx.with_mut(|i| i.processed = true);
    Ok(PipelineControl::Continue)
  });

  let results = FanOut::new(Arc::new(p))
    .spawner(Arc::new(CountingSpawner {
      spawns: Arc::new(AtomicUsize::new(0)),
    }))
    .run(items(4, 0))
    .await;

  assert_eq!(results.succeeded(), 3, "the siblings are unaffected");
  assert_eq!(results.failed(), 1);

  let (index, error) = results.errors().next().expect("the panicking branch is reported");
  assert_eq!(index, 1);
  // TestError stringifies framework errors with `{:?}`, so the variant is what surfaces.
  assert!(
    error.to_string().contains("FanOutBranchLost") && error.to_string().contains("index: 1"),
    "got: {}",
    error
  );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fail_fast_with_a_spawner_never_spawns_the_skipped_branches() {
  setup_tracing();
  let spawns = Arc::new(AtomicUsize::new(0));

  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", |ctx: ContextData<Item>| async move {
    let (id, work_ms) = ctx.with_ref(|i| (i.id, i.work_ms));
    tokio::time::sleep(Duration::from_millis(work_ms)).await;
    if id == 0 {
      return Err(TestError::Handler("first fails".into()));
    }
    Ok(PipelineControl::Continue)
  });

  let ladder = vec![
    Item { id: 0, work_ms: 0, processed: false },
    Item { id: 1, work_ms: 20, processed: false },
    Item { id: 2, work_ms: 0, processed: false },
    Item { id: 3, work_ms: 0, processed: false },
  ];

  let results = FanOut::new(Arc::new(p))
    .spawner(Arc::new(CountingSpawner { spawns: spawns.clone() }))
    .policy(FanOutPolicy::FailFast)
    .max_concurrent(2)
    .run(ladder)
    .await;

  assert_eq!(results.failed(), 1);
  assert_eq!(results.not_started(), 2, "items 2 and 3 never started");
  assert_eq!(
    spawns.load(Ordering::SeqCst),
    2,
    "and so were never spawned: only the two that started became tasks"
  );
}

/// The batteries-included spawner, so the common case needs no consumer glue at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_tokio_spawner_works_out_of_the_box() {
  setup_tracing();
  let probe = ConcurrencyProbe::default();

  let results = FanOut::new(probed_pipeline(probe.clone()))
    .spawner(Arc::new(orka::TokioSpawner))
    .max_concurrent(3)
    .run(items(9, 5))
    .await;

  assert_eq!(results.succeeded(), 9);
  assert!(probe.peak() <= 3, "the cap still holds, saw {}", probe.peak());
  let ids: Vec<usize> = results.items().iter().map(|i| i.context.with_ref(|it| it.id)).collect();
  assert_eq!(ids, (0..9).collect::<Vec<_>>());
}

/// Cancelling stops new branches from starting but never drops one already running, so an
/// in-flight branch still reaches its own finish ring.
#[tokio::test]
async fn cancelling_drains_in_flight_branches_instead_of_dropping_them() {
  setup_tracing();
  let finished = Arc::new(Mutex::new(Vec::<usize>::new()));
  let token = CancelToken::new();

  let canceller = token.clone();
  let mut p: Pipeline<Item, TestError> = Pipeline::new(["work"]);
  p.on_root("work", move |ctx: ContextData<Item>| {
    let canceller = canceller.clone();
    async move {
      let (id, work_ms) = ctx.with_ref(|i| (i.id, i.work_ms));
      // Cancelling after the sleep, so items 1 and 2 are already parked mid-handler and
      // are genuinely in flight rather than merely allocated a slot.
      tokio::time::sleep(Duration::from_millis(work_ms)).await;
      if id == 0 {
        canceller.cancel();
      }
      ctx.with_mut(|i| i.processed = true);
      Ok(PipelineControl::Continue)
    }
  });

  let recorder = finished.clone();
  p.on_finish(move |ctx: ContextData<Item>, _outcome| {
    let recorder = recorder.clone();
    async move {
      recorder.lock().unwrap().push(ctx.with_ref(|i| i.id));
      Ok(())
    }
  });

  let ladder = vec![
    Item { id: 0, work_ms: 5, processed: false },
    Item { id: 1, work_ms: 40, processed: false },
    Item { id: 2, work_ms: 40, processed: false },
    Item { id: 3, work_ms: 0, processed: false },
    Item { id: 4, work_ms: 0, processed: false },
  ];

  let results = FanOut::new(Arc::new(p))
    .with_cancel(token)
    .max_concurrent(3)
    .run(ladder)
    .await;

  assert!(results.was_cancelled());
  assert_eq!(results.not_started(), 2, "items 3 and 4 never started");

  let mut drained = finished.lock().unwrap().clone();
  drained.sort();
  assert_eq!(
    drained,
    vec![0, 1, 2],
    "the three in-flight branches each ran their own finish ring"
  );
  assert!(
    results.items()[1].context.with_ref(|i| i.processed),
    "and an in-flight branch ran to its own end rather than being dropped"
  );
}

/// The distinction a caller acts on: a branch that never started has nothing to tear down,
/// one interrupted mid-run does.
#[tokio::test]
async fn cancelled_branches_are_reported_apart_from_never_started_ones() {
  setup_tracing();
  let token = CancelToken::new();
  let canceller = token.clone();

  let mut p: Pipeline<Item, TestError> = Pipeline::new(["register", "finish"]);
  p.on_root("register", move |ctx: ContextData<Item>| {
    let canceller = canceller.clone();
    async move {
      ctx.with_mut(|i| i.processed = true);
      if ctx.with_ref(|i| i.id) == 0 {
        canceller.cancel();
      }
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("finish", |_ctx: ContextData<Item>| async move {
    Ok(PipelineControl::Continue)
  });

  let results = FanOut::new(Arc::new(p))
    .with_cancel(token)
    .max_concurrent(2)
    .run(items(6, 0))
    .await;

  assert!(results.was_cancelled());
  assert_eq!(results.cancelled(), 2, "both in-flight branches were interrupted");
  assert_eq!(results.not_started(), 4);
  assert_eq!(results.succeeded(), 0);
  assert_eq!(results.failed(), 0);

  assert!(
    results.items()[0].context.with_ref(|i| i.processed),
    "an interrupted branch did real work"
  );
  assert!(
    !results.items()[5].context.with_ref(|i| i.processed),
    "one that never started did not"
  );
}

#[tokio::test]
async fn a_cancelled_fanout_is_unmet_even_under_collect_all() {
  setup_tracing();
  let token = CancelToken::new();
  token.cancel();

  let results = FanOut::new(probed_pipeline(ConcurrencyProbe::default()))
    .policy(FanOutPolicy::CollectAll)
    .with_cancel(token)
    .run(items(4, 0))
    .await;

  assert!(results.was_cancelled());
  assert!(!results.satisfied(), "CollectAll claims everything ran, and it did not");
  assert_eq!(results.not_started(), 4);
  assert_eq!(
    results.into_control().unwrap(),
    PipelineControl::Stop,
    "cancellation is an outcome, not a branch failure"
  );
}

/// The case an ambient thread-local design would have missed: every branch is on its own
/// task, so the token has to travel in the context.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_reaches_branches_running_on_their_own_tasks() {
  setup_tracing();
  let token = CancelToken::new();
  let canceller = token.clone();

  let mut p: Pipeline<Item, TestError> = Pipeline::new(["first", "second"]);
  p.on_root("first", move |ctx: ContextData<Item>| {
    let canceller = canceller.clone();
    async move {
      if ctx.with_ref(|i| i.id) == 0 {
        canceller.cancel();
      }
      Ok(PipelineControl::Continue)
    }
  })
  .on_root("second", |ctx: ContextData<Item>| async move {
    ctx.with_mut(|i| i.processed = true);
    Ok(PipelineControl::Continue)
  });

  let results = FanOut::new(Arc::new(p))
    .spawner(Arc::new(orka::TokioSpawner))
    .with_cancel(token)
    .max_concurrent(2)
    .run(items(6, 0))
    .await;

  assert!(results.was_cancelled());
  assert_eq!(results.succeeded(), 0, "no branch got past its boundary check");
  assert!(
    results.items().iter().all(|i| !i.context.with_ref(|it| it.processed)),
    "the second step never ran on any branch"
  );
}
