//! Execution observation: the [`PipelineObserver`] trait, the [`TraceEvent`] stream a
//! running pipeline emits, and [`TraceCollector`], the batteries-included buffering
//! observer used for assertions in tests.
//!
//! The primitive is the trait. A pipeline holds at most one observer (attach with
//! [`Pipeline::set_observer`](crate::Pipeline::set_observer) or the
//! [`set_tracer`](crate::Pipeline::set_tracer) convenience); every run then reports its
//! progress as a series of [`TraceEvent`]s. `TraceCollector` is just one implementation
//! that buffers events into a shared `Vec`; a production observer (a tracing-span bridge,
//! a metrics counter) is another `impl PipelineObserver` with no buffer growth at all.

use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

/// Which handler vector within a step an event refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum StepPhase {
  Before,
  On,
  After,
}

impl fmt::Display for StepPhase {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      StepPhase::Before => write!(f, "before"),
      StepPhase::On => write!(f, "on"),
      StepPhase::After => write!(f, "after"),
    }
  }
}

/// Why a step was skipped rather than executed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum SkipReason {
  /// The step's `skip_if` predicate returned `true`. `label` carries the human-readable
  /// name given via [`Pipeline::skip_if_labeled`](crate::Pipeline::skip_if_labeled)
  /// ("drain disabled by config"), `None` for a plain `skip_if`.
  SkipCondition { label: Option<String> },
  /// The step is optional and has no handlers registered.
  OptionalWithoutHandlers,
}

impl fmt::Display for SkipReason {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      SkipReason::SkipCondition { label: Some(label) } => write!(f, "{}", label),
      SkipReason::SkipCondition { label: None } => write!(f, "skip_if condition"),
      SkipReason::OptionalWithoutHandlers => write!(f, "optional without handlers"),
    }
  }
}

/// How a single handler invocation ended.
///
/// Errors are captured as strings because pipeline `Err` types are not required to be
/// `Clone`. For type-level assertions on the live error, implement
/// [`PipelineObserver::on_handler_error`], which is called with the borrowed error before
/// it is stringified into the buffered event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum HandlerOutcome {
  Continue,
  Stop,
  Error(String),
}

impl fmt::Display for HandlerOutcome {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      HandlerOutcome::Continue => write!(f, "continue"),
      HandlerOutcome::Stop => write!(f, "stop"),
      HandlerOutcome::Error(e) => write!(f, "error: {}", e),
    }
  }
}

/// The final outcome of a pipeline run, as reported to finish handlers and in
/// [`TraceEventKind::RunFinished`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum RunOutcome {
  Completed,
  Stopped,
  Errored { step: String, message: String },
}

impl fmt::Display for RunOutcome {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      RunOutcome::Completed => write!(f, "completed"),
      RunOutcome::Stopped => write!(f, "stopped"),
      RunOutcome::Errored { step, message } => write!(f, "errored at '{}': {}", step, message),
    }
  }
}

/// What happened, without the run id. See [`TraceEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum TraceEventKind {
  /// A full `run()` started. Partial runs (`run_step`/`run_from`/`run_until`) do not emit
  /// this: they are inspection tools, not runs, so their traces contain step events only.
  RunStarted,
  /// A step passed its skip checks and is about to execute its handlers.
  StepStarted { step: String, index: usize },
  /// A step was skipped without executing any handler.
  StepSkipped {
    step: String,
    index: usize,
    reason: SkipReason,
  },
  /// A single handler invocation finished.
  HandlerFinished {
    step: String,
    phase: StepPhase,
    handler_index: usize,
    outcome: HandlerOutcome,
  },
  /// All of a step's handlers ran to completion with `Continue`.
  StepCompleted { step: String, index: usize },
  /// A conditional step's master handler matched a scope (0-based, in registration order).
  ScopeMatched { step: String, scope_index: usize },
  /// A conditional step's master handler matched no scope and used its no-match behavior.
  ScopeNotMatched { step: String },
  /// A finish handler registered via `on_finish` finished (only full `run()` fires these).
  FinalizerFinished {
    handler_index: usize,
    outcome: HandlerOutcome,
  },
  /// Run-scoped resources were released, after the finish handlers. Emitted only when the
  /// bag was non-empty, so traces of pipelines that hold nothing are unchanged.
  ResourcesReleased { count: usize },
  /// A full `run()` finished, after any finish handlers. Partial runs do not emit this.
  RunFinished { outcome: RunOutcome },
}

impl fmt::Display for TraceEventKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      TraceEventKind::RunStarted => write!(f, "run started"),
      TraceEventKind::StepStarted { step, index } => write!(f, "step '{}' started (index {})", step, index),
      TraceEventKind::StepSkipped { step, index, reason } => {
        write!(f, "step '{}' skipped (index {}): {}", step, index, reason)
      }
      TraceEventKind::HandlerFinished {
        step,
        phase,
        handler_index,
        outcome,
      } => write!(f, "step '{}' {} handler #{}: {}", step, phase, handler_index, outcome),
      TraceEventKind::StepCompleted { step, index } => write!(f, "step '{}' completed (index {})", step, index),
      TraceEventKind::ScopeMatched { step, scope_index } => {
        write!(f, "step '{}' matched conditional scope #{}", step, scope_index)
      }
      TraceEventKind::ScopeNotMatched { step } => write!(f, "step '{}' matched no conditional scope", step),
      TraceEventKind::FinalizerFinished { handler_index, outcome } => {
        write!(f, "finish handler #{}: {}", handler_index, outcome)
      }
      TraceEventKind::ResourcesReleased { count } => write!(f, "released {} run-scoped resource(s)", count),
      TraceEventKind::RunFinished { outcome } => write!(f, "run finished: {}", outcome),
    }
  }
}

/// One observed event, tagged with the run that produced it.
///
/// `run_id` is allocated from a process-global counter at the start of every run (full or
/// partial), so events from concurrent runs of a shared pipeline interleave in a
/// [`TraceCollector`] but remain distinguishable; filter with
/// [`TraceCollector::for_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TraceEvent {
  pub run_id: u64,
  pub kind: TraceEventKind,
}

impl fmt::Display for TraceEvent {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "[run {}] {}", self.run_id, self.kind)
  }
}

/// An observer of pipeline execution.
///
/// Attach one with [`Pipeline::set_observer`](crate::Pipeline::set_observer). The
/// observer is snapshotted once at the start of each run, so attaching while a run is in
/// flight misses that run and catches the next one; this matters when attaching late to a
/// live shared pipeline, for example one obtained through
/// [`Orka::pipeline`](crate::Orka::pipeline).
pub trait PipelineObserver: Send + Sync {
  /// Called synchronously at each event site, before execution proceeds. Must not block:
  /// it runs on the pipeline's hot path, inside the async executor.
  fn on_event(&self, event: &TraceEvent);

  /// Called at handler-failure time with the live, borrowed error, before it is
  /// stringified into the buffered [`HandlerOutcome::Error`]. Since pipeline `Err` types
  /// are not `Clone`, this is the only place an observer can inspect the concrete error
  /// (via `error.downcast_ref::<MyError>()`). Default is a no-op.
  fn on_handler_error(&self, run_id: u64, step: &str, phase: StepPhase, error: &(dyn std::error::Error + 'static)) {
    let _ = (run_id, step, phase, error);
  }
}

/// A shared observer, whether attached to a pipeline or scoped to one call.
pub(crate) type SharedObserver = Arc<dyn PipelineObserver>;

/// The observer attachment slot on a pipeline. Shared (`Arc`) so closures that captured it
/// before an observer was attached (conditional master handlers) still see a later
/// attachment.
pub(crate) type ObserverSlot = Arc<Mutex<Option<Arc<dyn PipelineObserver>>>>;

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

/// Allocates a fresh, process-globally unique run id.
pub(crate) fn next_run_id() -> u64 {
  NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
  /// The run id of the handler future currently being polled on this thread (0 = none).
  /// Set around every handler poll by [`HandlerScope`], so code that runs inside a handler
  /// but far from the execution loop (the conditional master handler emitting scope
  /// events) can tag its events with the correct run.
  static CURRENT_RUN_ID: Cell<u64> = const { Cell::new(0) };

  /// The observer scoped to the *call* currently in flight, as opposed to one attached to
  /// a pipeline. Set around every handler poll so that runs started from inside a handler,
  /// namely fan-out branches and conditional sub-pipelines, inherit it and report into the
  /// same collector as their parent.
  static CURRENT_SCOPED_OBSERVER: RefCell<Option<Arc<dyn PipelineObserver>>> =
    const { RefCell::new(None) };
}

/// The run id of the handler currently executing on this thread, or 0 outside one.
pub(crate) fn current_run_id() -> u64 {
  CURRENT_RUN_ID.with(|c| c.get())
}

/// The call-scoped observer in force on this thread, if a handler above us was started by
/// a run that was given one.
pub(crate) fn current_scoped_observer() -> Option<SharedObserver> {
  CURRENT_SCOPED_OBSERVER.with(|c| c.borrow().clone())
}

/// Combines two optional observers, so a pipeline-attached one and a call-scoped one both
/// see every event rather than one displacing the other.
pub(crate) fn combine_observers(a: Option<SharedObserver>, b: Option<SharedObserver>) -> Option<SharedObserver> {
  match (a, b) {
    (None, None) => None,
    (Some(only), None) | (None, Some(only)) => Some(only),
    (Some(first), Some(second)) => Some(Arc::new(CompositeObserver::with(vec![first, second]))),
  }
}

/// Wraps a handler future so that [`current_run_id`] and [`current_scoped_observer`] are
/// set for the duration of each poll. The poll itself is synchronous, so this survives
/// executor thread migration: whichever thread polls, that thread's slots are set first
/// and restored after.
///
/// Both epilogues **restore the previous value** rather than clearing, which matters as
/// soon as runs nest: a handler that runs a sub-pipeline (a conditional scope, a fan-out
/// branch) polls the child's scope inside its own poll, and clearing there would leave the
/// parent's ambient state lost for the remainder of that poll.
pub(crate) struct HandlerScope<F> {
  pub(crate) run_id: u64,
  pub(crate) scoped_observer: Option<Arc<dyn PipelineObserver>>,
  pub(crate) fut: F,
}

impl<F: Future + Unpin> Future for HandlerScope<F> {
  type Output = F::Output;

  fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
    let run_id = self.run_id;
    let scoped = self.scoped_observer.clone();

    let previous_run_id = CURRENT_RUN_ID.with(|c| c.replace(run_id));
    let previous_observer = CURRENT_SCOPED_OBSERVER.with(|c| c.replace(scoped));

    let result = Pin::new(&mut self.fut).poll(cx);

    CURRENT_RUN_ID.with(|c| c.set(previous_run_id));
    CURRENT_SCOPED_OBSERVER.with(|c| *c.borrow_mut() = previous_observer);
    result
  }
}

/// A cheaply cloneable, thread-safe buffer of [`TraceEvent`]s implementing
/// [`PipelineObserver`].
///
/// Keep a clone in your test, attach the other to the pipeline:
///
/// ```ignore
/// let trace = TraceCollector::new();
/// pipeline.set_tracer(trace.clone());
/// pipeline.run(ctx).await?;
/// assert!(trace.step_completed("load"));
/// ```
///
/// This is an accumulating log: it grows for as long as it stays attached and is never
/// cleared. That is what tests want; for production observability prefer a streaming
/// [`PipelineObserver`] implementation (or call [`clear`](Self::clear) periodically).
///
/// The flat query helpers ([`completed_steps`](Self::completed_steps),
/// [`last_outcome`](Self::last_outcome), ...) read the whole buffer, which is only
/// unambiguous when a single run was recorded. Concurrent runs of a shared pipeline
/// interleave their events; scope queries to one run with [`for_run`](Self::for_run):
///
/// ```ignore
/// let run = trace.for_run(*trace.run_ids().last().unwrap());
/// assert_eq!(run.last_outcome(), Some(RunOutcome::Completed));
/// ```
#[derive(Clone, Default)]
pub struct TraceCollector {
  inner: Arc<Mutex<Vec<TraceEvent>>>,
}

impl TraceCollector {
  pub fn new() -> Self {
    Self::default()
  }

  /// Appends one event. Public so custom code (test fixtures, custom runners) can
  /// participate in the same log.
  pub fn record(&self, event: TraceEvent) {
    self.inner.lock().push(event);
  }

  /// A snapshot of all recorded events, in order.
  pub fn events(&self) -> Vec<TraceEvent> {
    self.inner.lock().clone()
  }

  /// Discards all recorded events.
  pub fn clear(&self) {
    self.inner.lock().clear();
  }

  /// The distinct run ids seen, in order of first appearance.
  pub fn run_ids(&self) -> Vec<u64> {
    let events = self.inner.lock();
    let mut ids: Vec<u64> = Vec::new();
    for e in events.iter() {
      if !ids.contains(&e.run_id) {
        ids.push(e.run_id);
      }
    }
    ids
  }

  /// A snapshot view scoped to a single run, exposing the same query helpers filtered to
  /// that run's events.
  pub fn for_run(&self, run_id: u64) -> RunTrace {
    let events = self
      .inner
      .lock()
      .iter()
      .filter(|e| e.run_id == run_id)
      .cloned()
      .collect();
    RunTrace { run_id, events }
  }

  /// Names of steps that completed, in order. Unfiltered across runs; see [`for_run`](Self::for_run).
  pub fn completed_steps(&self) -> Vec<String> {
    completed_steps(&self.inner.lock())
  }

  /// Names of steps that were skipped, in order. Unfiltered across runs; see [`for_run`](Self::for_run).
  pub fn skipped_steps(&self) -> Vec<String> {
    skipped_steps(&self.inner.lock())
  }

  /// Whether the named step completed in any recorded run.
  pub fn step_completed(&self, step: &str) -> bool {
    self.completed_steps().iter().any(|s| s == step)
  }

  /// Whether the named step was skipped in any recorded run.
  pub fn step_skipped(&self, step: &str) -> bool {
    self.skipped_steps().iter().any(|s| s == step)
  }

  /// Outcomes of every handler invocation for a (step, phase), in order.
  pub fn handler_finishes(&self, step: &str, phase: StepPhase) -> Vec<HandlerOutcome> {
    handler_finishes(&self.inner.lock(), step, phase)
  }

  /// Number of full runs recorded (count of `RunStarted` events).
  pub fn run_count(&self) -> usize {
    self
      .inner
      .lock()
      .iter()
      .filter(|e| matches!(e.kind, TraceEventKind::RunStarted))
      .count()
  }

  /// The outcome of the most recently finished full run, if any.
  pub fn last_outcome(&self) -> Option<RunOutcome> {
    last_outcome(&self.inner.lock())
  }
}

impl PipelineObserver for TraceCollector {
  fn on_event(&self, event: &TraceEvent) {
    self.record(event.clone());
  }
}

/// Fans out to multiple observers, in order.
///
/// A pipeline's observer slot deliberately holds a single observer; when a production
/// bridge (metrics, tracing) and a diagnostic [`TraceCollector`] must coexist, compose
/// them with this instead of displacing one another:
///
/// ```ignore
/// let mut composite = CompositeObserver::new();
/// composite.push(Arc::new(metrics_bridge));
/// composite.push(Arc::new(trace.clone()));
/// pipeline.set_observer(Arc::new(composite));
/// ```
#[derive(Clone, Default)]
pub struct CompositeObserver {
  observers: Vec<Arc<dyn PipelineObserver>>,
}

impl CompositeObserver {
  pub fn new() -> Self {
    Self::default()
  }

  /// Builds a composite from an existing list.
  pub fn with(observers: Vec<Arc<dyn PipelineObserver>>) -> Self {
    Self { observers }
  }

  /// Appends an observer; it receives events after those added before it.
  pub fn push(&mut self, observer: Arc<dyn PipelineObserver>) -> &mut Self {
    self.observers.push(observer);
    self
  }
}

impl PipelineObserver for CompositeObserver {
  fn on_event(&self, event: &TraceEvent) {
    for observer in &self.observers {
      observer.on_event(event);
    }
  }

  fn on_handler_error(&self, run_id: u64, step: &str, phase: StepPhase, error: &(dyn std::error::Error + 'static)) {
    for observer in &self.observers {
      observer.on_handler_error(run_id, step, phase, error);
    }
  }
}

/// A snapshot of one run's events, produced by [`TraceCollector::for_run`].
#[derive(Debug, Clone)]
pub struct RunTrace {
  run_id: u64,
  events: Vec<TraceEvent>,
}

impl RunTrace {
  pub fn run_id(&self) -> u64 {
    self.run_id
  }

  pub fn events(&self) -> &[TraceEvent] {
    &self.events
  }

  pub fn completed_steps(&self) -> Vec<String> {
    completed_steps(&self.events)
  }

  pub fn skipped_steps(&self) -> Vec<String> {
    skipped_steps(&self.events)
  }

  pub fn step_completed(&self, step: &str) -> bool {
    self.completed_steps().iter().any(|s| s == step)
  }

  pub fn step_skipped(&self, step: &str) -> bool {
    self.skipped_steps().iter().any(|s| s == step)
  }

  pub fn handler_finishes(&self, step: &str, phase: StepPhase) -> Vec<HandlerOutcome> {
    handler_finishes(&self.events, step, phase)
  }

  pub fn last_outcome(&self) -> Option<RunOutcome> {
    last_outcome(&self.events)
  }
}

fn completed_steps(events: &[TraceEvent]) -> Vec<String> {
  events
    .iter()
    .filter_map(|e| match &e.kind {
      TraceEventKind::StepCompleted { step, .. } => Some(step.clone()),
      _ => None,
    })
    .collect()
}

fn skipped_steps(events: &[TraceEvent]) -> Vec<String> {
  events
    .iter()
    .filter_map(|e| match &e.kind {
      TraceEventKind::StepSkipped { step, .. } => Some(step.clone()),
      _ => None,
    })
    .collect()
}

fn handler_finishes(events: &[TraceEvent], step: &str, phase: StepPhase) -> Vec<HandlerOutcome> {
  events
    .iter()
    .filter_map(|e| match &e.kind {
      TraceEventKind::HandlerFinished {
        step: s,
        phase: p,
        outcome,
        ..
      } if s == step && *p == phase => Some(outcome.clone()),
      _ => None,
    })
    .collect()
}

fn last_outcome(events: &[TraceEvent]) -> Option<RunOutcome> {
  events.iter().rev().find_map(|e| match &e.kind {
    TraceEventKind::RunFinished { outcome } => Some(outcome.clone()),
    _ => None,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  /// A nested run must restore its parent's ambient run id rather than clearing it.
  /// Without the restore, everything the parent emits after awaiting a sub-pipeline is
  /// tagged `run_id: 0` and dropped by [`TraceCollector::for_run`].
  #[tokio::test]
  async fn nested_with_run_id_restores_the_parent_id() {
    let after_nested = Arc::new(Mutex::new(u64::MAX));
    let seen = after_nested.clone();

    let inner = Box::pin(async {
      assert_eq!(current_run_id(), 7, "the nested run sees its own id");
    });
    let outer = Box::pin(async move {
      assert_eq!(current_run_id(), 42, "the parent sees its own id before nesting");
      HandlerScope { run_id: 7, scoped_observer: None, fut: inner }.await;
      *seen.lock() = current_run_id();
    });

    HandlerScope { run_id: 42, scoped_observer: None, fut: outer }.await;

    assert_eq!(*after_nested.lock(), 42, "the parent's id survives a nested run");
    assert_eq!(current_run_id(), 0, "and the top level is left clear");
  }
}
