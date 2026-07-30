//! Tests for out-of-band cancellation: the token itself, the step-boundary check, and the
//! guarantee that a cancelled run still takes its normal exit through the finish ring and
//! the resource bag.

mod common;

use common::{setup_tracing, ScopedTestContextA, TestContext, TestError};
use orka::test_util::{assert_run_outcome, ExecutionCounter};
use orka::{
  CancelToken, ContextData, Pipeline, PipelineControl, PipelineResult, PlannedAction, RunOutcome, TraceCollector,
  TraceEventKind,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn three_step_pipeline() -> Pipeline<TestContext, TestError> {
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["alpha", "beta", "gamma"]);
  for step in ["alpha", "beta", "gamma"] {
    p.on_root(step, move |ctx: ContextData<TestContext>| async move {
      ctx.write().steps_executed.push(step.into());
      Ok(PipelineControl::Continue)
    });
  }
  p
}

// --- The token itself ---

#[tokio::test]
async fn cancelled_resolves_for_a_token_cancelled_before_the_await() {
  let token = CancelToken::new();
  token.cancel();
  token.cancelled().await;
}

#[tokio::test]
async fn cancelled_resolves_for_a_token_cancelled_during_the_await() {
  let token = CancelToken::new();
  let watcher = token.clone();
  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(10)).await;
    watcher.cancel();
  });

  tokio::time::timeout(Duration::from_secs(5), token.cancelled())
    .await
    .expect("a cancel from another task wakes the waiter");
}

/// The residual that step-boundary polling alone leaves: a handler parked on a long await
/// closes it by racing the token against its own work.
#[tokio::test]
async fn a_handler_can_select_between_its_work_and_the_token() {
  let token = CancelToken::new();
  let watcher = token.clone();
  tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(10)).await;
    watcher.cancel();
  });

  let picked_cancel = tokio::select! {
    _ = token.cancelled() => true,
    _ = tokio::time::sleep(Duration::from_secs(30)) => false,
  };

  assert!(picked_cancel, "the cancel arm won against a 30s wait");
}

#[tokio::test]
async fn cancel_is_idempotent() {
  let token = CancelToken::new();
  assert!(!token.is_cancelled());
  token.cancel();
  token.cancel();
  assert!(token.is_cancelled());
}

/// `cancel` empties the waker vector, so a `Cancelled` still alive at that moment holds an
/// index into a vector that no longer has it. Dropping it must not index.
#[tokio::test]
async fn dropping_a_registered_waiter_after_cancellation_does_not_panic() {
  let token = CancelToken::new();

  let mut waiter = Box::pin(token.cancelled());
  let poll = futures_lite_poll_once(waiter.as_mut()).await;
  assert!(poll.is_none(), "not cancelled yet, so it parks and registers a waker");

  token.cancel();
  drop(waiter);
}

/// Polls `fut` exactly once, returning `None` if it parked. Hand-rolled because orka's
/// tests carry no futures utility crate.
async fn futures_lite_poll_once<F: std::future::Future>(fut: std::pin::Pin<&mut F>) -> Option<F::Output> {
  struct PollOnce<'a, F>(Option<std::pin::Pin<&'a mut F>>);

  impl<F: std::future::Future> std::future::Future for PollOnce<'_, F> {
    type Output = Option<F::Output>;

    fn poll(
      mut self: std::pin::Pin<&mut Self>,
      cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
      let mut inner = self.0.take().expect("polled after completion");
      match inner.as_mut().poll(cx) {
        std::task::Poll::Ready(v) => std::task::Poll::Ready(Some(v)),
        std::task::Poll::Pending => std::task::Poll::Ready(None),
      }
    }
  }

  PollOnce(Some(fut)).await
}

// --- The step-boundary check ---

#[tokio::test]
async fn a_cancelled_run_stops_before_its_next_step() {
  setup_tracing();
  let mut p = three_step_pipeline();
  let token = CancelToken::new();

  let canceller = token.clone();
  p.after_root("alpha", move |_ctx: ContextData<TestContext>| {
    let canceller = canceller.clone();
    async move {
      canceller.cancel();
      Ok(PipelineControl::Continue)
    }
  });

  let ctx = ContextData::new(TestContext::default());
  let result = p.run_with_cancel(ctx.clone(), token).await.unwrap();

  assert_eq!(result, PipelineResult::Cancelled);
  assert_eq!(
    ctx.read().steps_executed,
    vec!["alpha"],
    "beta and gamma were never started"
  );
}

#[tokio::test]
async fn an_uncancelled_run_is_unaffected() {
  setup_tracing();
  let p = three_step_pipeline();
  let ctx = ContextData::new(TestContext::default());

  let result = p.run_with_cancel(ctx.clone(), CancelToken::new()).await.unwrap();

  assert_eq!(result, PipelineResult::Completed);
  assert_eq!(ctx.read().steps_executed, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn a_handler_returning_stop_under_cancellation_reports_cancelled() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["alpha"]);
  let token = CancelToken::new();

  p.on_root("alpha", |ctx: ContextData<TestContext>| async move {
    ctx.cancellation().cancel();
    Ok(PipelineControl::Stop)
  });

  let (result, outcome) = p
    .run_with_cancel_and_outcome(ContextData::new(TestContext::default()), token)
    .await;

  assert_eq!(result.unwrap(), PipelineResult::Cancelled);
  assert_eq!(outcome, RunOutcome::Cancelled);
}

#[tokio::test]
async fn a_plain_stop_still_reports_stopped() {
  setup_tracing();
  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["alpha", "beta"]);
  p.on_root("alpha", |_ctx: ContextData<TestContext>| async move {
    Ok(PipelineControl::Stop)
  });

  let (result, outcome) = p
    .run_with_cancel_and_outcome(ContextData::new(TestContext::default()), CancelToken::new())
    .await;

  assert_eq!(result.unwrap(), PipelineResult::Stopped);
  assert_eq!(outcome, RunOutcome::Stopped);
}

// --- The exit path ---

/// The load-bearing guarantee: cancellation is a wind-down, not a drop, so a cancelled run
/// cleans up exactly as a completed one does.
#[tokio::test]
async fn a_cancelled_run_still_fires_on_finish_and_releases_resources() {
  setup_tracing();
  let mut p = three_step_pipeline();
  let token = CancelToken::new();

  let canceller = token.clone();
  p.before_root("beta", move |_ctx: ContextData<TestContext>| {
    let canceller = canceller.clone();
    async move {
      canceller.cancel();
      Ok(PipelineControl::Continue)
    }
  });

  p.on_root("alpha", |ctx: ContextData<TestContext>| async move {
    ctx.resources().put(String::from("held"));
    ctx.write().steps_executed.push("alpha".into());
    Ok(PipelineControl::Continue)
  });

  let finish_ran = ExecutionCounter::new();
  let seen_outcome = ContextData::new(Vec::<RunOutcome>::new());
  let counter = finish_ran.clone();
  let outcomes = seen_outcome.clone();
  p.on_finish(move |ctx: ContextData<TestContext>, outcome| {
    let counter = counter.clone();
    let outcomes = outcomes.clone();
    async move {
      counter.increment();
      outcomes.write().push(outcome);
      assert!(
        ctx.resources().with(|s: &String| s.clone()).is_some(),
        "the finish ring runs before the bag is released"
      );
      Ok(())
    }
  });

  let ctx = ContextData::new(TestContext::default());
  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());

  let result = p.run_with_cancel(ctx.clone(), token).await.unwrap();

  assert_eq!(result, PipelineResult::Cancelled);
  assert_eq!(finish_ran.get(), 1, "the finish ring fired");
  assert_eq!(seen_outcome.read().as_slice(), &[RunOutcome::Cancelled]);
  assert!(ctx.resources().is_empty(), "and the bag was released after it");
  assert!(
    trace
      .events()
      .iter()
      .any(|e| matches!(&e.kind, TraceEventKind::ResourcesReleased { count } if *count == 1)),
    "resource release is reported, exactly as on a completed run"
  );
}

/// A cancel raised once the finish ring has started is ignored. The ring is the cleanup a
/// cancelled run exists to reach, so interrupting it would strand what the finalizers are
/// there to unwind.
#[tokio::test]
async fn cancellation_during_the_finish_ring_does_not_interrupt_it() {
  setup_tracing();
  let mut p = three_step_pipeline();
  let token = CancelToken::new();
  let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

  let canceller = token.clone();
  let first = order.clone();
  p.on_finish(move |_ctx: ContextData<TestContext>, _outcome| {
    let canceller = canceller.clone();
    let order = first.clone();
    async move {
      order.lock().push("first");
      canceller.cancel();
      Ok(())
    }
  });

  let second = order.clone();
  p.on_finish(move |_ctx: ContextData<TestContext>, _outcome| {
    let order = second.clone();
    async move {
      order.lock().push("second");
      Ok(())
    }
  });

  let third = order.clone();
  p.on_finish(move |_ctx: ContextData<TestContext>, _outcome| {
    let order = third.clone();
    async move {
      order.lock().push("third");
      Ok(())
    }
  });

  let ctx = ContextData::new(TestContext::default());
  let result = p.run_with_cancel(ctx.clone(), token).await.unwrap();

  assert_eq!(result, PipelineResult::Completed, "the run itself was never cancelled");
  assert_eq!(
    order.lock().clone(),
    vec!["first", "second", "third"],
    "every finalizer after the cancelling one still ran"
  );
}

// --- Observation ---

#[tokio::test]
async fn a_cancelled_run_reports_where_it_stopped() {
  setup_tracing();
  let mut p = three_step_pipeline();
  let token = CancelToken::new();

  let canceller = token.clone();
  p.after_root("alpha", move |_ctx: ContextData<TestContext>| {
    let canceller = canceller.clone();
    async move {
      canceller.cancel();
      Ok(PipelineControl::Continue)
    }
  });

  let trace = TraceCollector::new();
  p.set_tracer(trace.clone());
  p.run_with_cancel(ContextData::new(TestContext::default()), token)
    .await
    .unwrap();

  let landed = trace.events().into_iter().find_map(|e| match e.kind {
    TraceEventKind::RunCancelled { step, index } => Some((step, index)),
    _ => None,
  });
  assert_eq!(
    landed,
    Some(("beta".to_string(), 1)),
    "the trace names the step the run got as far as"
  );
  assert_run_outcome(&trace, RunOutcome::Cancelled);
}

#[tokio::test]
async fn resolve_plan_against_a_cancelled_context_plans_nothing() {
  setup_tracing();
  let p = three_step_pipeline();
  let ctx = ContextData::new(TestContext::default());
  ctx.cancellation().cancel();

  let plan = p.resolve_plan(&ctx);

  assert_eq!(plan.len(), 3);
  assert!(
    plan.iter().all(|s| s.action == PlannedAction::Cancelled),
    "got {:?}",
    plan
  );
}

// --- Propagation ---

#[tokio::test]
async fn a_cancelled_parent_cancels_its_conditional_scope() {
  setup_tracing();
  let scope_steps = Arc::new(AtomicUsize::new(0));

  let counted = scope_steps.clone();
  let factory = move |_main: ContextData<TestContext>| {
    let counted = counted.clone();
    let mut sub = Pipeline::<ScopedTestContextA, TestError>::new(["first", "second"]);
    let a = counted.clone();
    sub.on_root("first", move |ctx: ContextData<ScopedTestContextA>| {
      let a = a.clone();
      async move {
        a.fetch_add(1, Ordering::SeqCst);
        ctx.cancellation().cancel();
        Ok(PipelineControl::Continue)
      }
    });
    let b = counted.clone();
    sub.on_root("second", move |_ctx: ContextData<ScopedTestContextA>| {
      let b = b.clone();
      async move {
        b.fetch_add(1, Ordering::SeqCst);
        Ok(PipelineControl::Continue)
      }
    });
    std::future::ready(Ok(Arc::new(sub)))
  };

  let mut p: Pipeline<TestContext, TestError> = Pipeline::new(["conditional_step", "after"]);
  p.conditional_scopes_for_step("conditional_step")
    .add_dynamic_scope(factory, |main: ContextData<TestContext>| {
      Ok(main.project(|_| ScopedTestContextA::default()))
    })
    .on_condition(|_| true)
    .finalize_conditional_step(false);
  p.on_root("after", |ctx: ContextData<TestContext>| async move {
    ctx.write().steps_executed.push("after".into());
    Ok(PipelineControl::Continue)
  });

  let ctx = ContextData::new(TestContext::default());
  let token = CancelToken::new();
  let (result, outcome) = p.run_with_cancel_and_outcome(ctx.clone(), token).await;

  assert_eq!(scope_steps.load(Ordering::SeqCst), 1, "the scope's second step never ran");
  assert_eq!(result.unwrap(), PipelineResult::Cancelled);
  assert_eq!(outcome, RunOutcome::Cancelled);
  assert!(
    ctx.read().steps_executed.is_empty(),
    "and the parent's next step never ran either"
  );
}
