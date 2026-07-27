//! Test utilities for consumers of orka (and for orka's own suite), behind the
//! `test-util` cargo feature.
//!
//! Enable it in your dev-dependencies alongside the normal dependency:
//!
//! ```toml
//! [dependencies]
//! orka = "0.2"
//!
//! [dev-dependencies]
//! orka = { version = "0.2", features = ["test-util"] }
//! ```
//!
//! What is here:
//! - [`TestError`]: a `Clone + PartialEq` pipeline error (real `OrkaError` is neither).
//! - [`ExecutionCounter`]: a local, cloneable call counter; the replacement for
//!   process-global atomics plus serial tests.
//! - Handler factories ([`continue_handler`], [`stop_handler`], [`fail_handler`],
//!   [`counting_handler`]) compatible with `on_root`/`before_root`/`after_root`.
//! - [`PipelineTestExt::fail_at`]: force a failure at a step, by name.
//! - [`noop_pipeline`]: a pipeline of continue-only steps, the canned shape for
//!   structural and skip-condition tests.
//! - [`MockPipeline`]: a canned [`PipelineRunner`] for faking a whole pipeline at the run
//!   boundary, for example via [`Orka::register_runner`](crate::Orka::register_runner).
//! - Trace assertion helpers ([`assert_steps_completed`], [`assert_order`], ...) that
//!   panic with readable diffs at the test's own line.

use crate::core::context_data::ContextData;
use crate::core::control::{PipelineControl, PipelineResult};
use crate::core::trace::{RunOutcome, TraceCollector};
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline;
use crate::pipeline::runner::PipelineRunner;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::future::{ready, Ready};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;


/// A ready-made pipeline error type for tests.
///
/// `OrkaError` is not `PartialEq` (its sources are `anyhow::Error`), so this error
/// stringifies framework errors via its `From<OrkaError>` impl to keep assertions
/// comparable with `assert_eq!`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TestError {
  #[error("Orka framework error: {0:?}")]
  Orka(String),

  #[error("Test handler failed: {0}")]
  Handler(String),

  #[error("Test extractor failed: {0}")]
  Extractor(String),

  #[error("Test pipeline provider failed: {0}")]
  Provider(String),

  #[error("Test scoped task failed: {0}")]
  ScopedTask(String),

  #[error("{0}")]
  Other(String),
}

impl From<OrkaError> for TestError {
  fn from(oe: OrkaError) -> Self {
    TestError::Orka(format!("{:?}", oe))
  }
}


/// A cloneable, thread-safe call counter.
///
/// Clone one into each closure whose invocations you want to count, assert locally. This
/// replaces the pattern of process-global `static` atomics, which force tests onto
/// `#[serial]`; an `ExecutionCounter` is scoped to the test that created it, so tests stay
/// parallel.
#[derive(Clone, Debug, Default)]
pub struct ExecutionCounter(Arc<AtomicUsize>);

impl ExecutionCounter {
  pub fn new() -> Self {
    Self::default()
  }

  /// Adds one and returns the new value.
  pub fn increment(&self) -> usize {
    self.0.fetch_add(1, Ordering::SeqCst) + 1
  }

  pub fn get(&self) -> usize {
    self.0.load(Ordering::SeqCst)
  }

  pub fn reset(&self) {
    self.0.store(0, Ordering::SeqCst);
  }
}

//
// These return closures (not boxed `Handler` values) so they slot directly into
// `on_root`/`before_root`/`after_root`/`replace_on_root`. Error factories are closures
// because pipeline `Err` types are not required to be `Clone`.

/// A handler that always continues.
pub fn continue_handler<TData, Err>() -> impl Fn(ContextData<TData>) -> Ready<Result<PipelineControl, Err>> + Send + Sync + 'static
where
  TData: 'static + Send + Sync,
{
  |_ctx| ready(Ok(PipelineControl::Continue))
}

/// A handler that always stops the pipeline.
pub fn stop_handler<TData, Err>() -> impl Fn(ContextData<TData>) -> Ready<Result<PipelineControl, Err>> + Send + Sync + 'static
where
  TData: 'static + Send + Sync,
{
  |_ctx| ready(Ok(PipelineControl::Stop))
}

/// A handler that always fails with the error produced by `make_err`.
pub fn fail_handler<TData, Err>(
  make_err: impl Fn() -> Err + Send + Sync + 'static,
) -> impl Fn(ContextData<TData>) -> Ready<Result<PipelineControl, Err>> + Send + Sync + 'static
where
  TData: 'static + Send + Sync,
{
  move |_ctx| ready(Err(make_err()))
}

/// A handler that continues, incrementing `counter` on every invocation.
pub fn counting_handler<TData, Err>(
  counter: ExecutionCounter,
) -> impl Fn(ContextData<TData>) -> Ready<Result<PipelineControl, Err>> + Send + Sync + 'static
where
  TData: 'static + Send + Sync,
{
  move |_ctx| {
    counter.increment();
    ready(Ok(PipelineControl::Continue))
  }
}


/// Test-oriented extension methods on [`Pipeline`].
pub trait PipelineTestExt<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Forces the pipeline to fail at the named step: replaces its `on` handlers with one
  /// that returns `make_err()`. A named wrapper over
  /// [`replace_on_root`](Pipeline::replace_on_root) + [`fail_handler`] for the common
  /// intent "make it break here, then assert what happens downstream".
  ///
  /// # Panics
  /// Panics if the step does not exist.
  fn fail_at(&mut self, step_name: impl AsRef<str>, make_err: impl Fn() -> Err + Send + Sync + 'static) -> &mut Self;
}

impl<TData, Err> PipelineTestExt<TData, Err> for Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  fn fail_at(&mut self, step_name: impl AsRef<str>, make_err: impl Fn() -> Err + Send + Sync + 'static) -> &mut Self {
    let step_name = step_name.as_ref();
    self.replace_on_root(step_name, fail_handler(make_err))
  }
}

/// A pipeline with the given steps, each carrying a single continue `on` handler.
///
/// The canned shape for structural and skip-condition tests: apply your real skip
/// conditions to it, seed a context, and either `resolve_plan` or run it with a
/// [`TraceCollector`] attached.
pub fn noop_pipeline<TData, Err, I, S>(step_names: I) -> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  I: IntoIterator<Item = S>,
  S: AsRef<str>,
{
  let names: Vec<String> = step_names.into_iter().map(|s| s.as_ref().to_string()).collect();
  let mut pipeline = Pipeline::new(&names);
  for name in &names {
    pipeline.on_root(name, continue_handler());
  }
  pipeline
}


enum CannedResponse<Err> {
  Completed,
  Stopped,
  Error(Box<dyn Fn() -> Err + Send + Sync>),
}

type BaseBehavior<TData, Err> = Box<dyn Fn(ContextData<TData>) -> Result<PipelineResult, Err> + Send + Sync>;

/// A canned run-boundary fake implementing [`PipelineRunner`].
///
/// Use it wherever an `Arc<dyn PipelineRunner<TData, Err>>` is accepted, most notably
/// [`Orka::register_runner`](crate::Orka::register_runner), so app-level tests can drive
/// the `Completed`/`Stopped`/`Err` match in their calling code without registering any
/// real pipeline:
///
/// ```ignore
/// let mut mock = MockPipeline::<CheckoutCtx, AppError>::completed();
/// mock.then_stopped(); // first run: Stopped; later runs: base behavior (Completed)
/// orka.register_runner::<CheckoutCtx, AppError>(Arc::new(mock));
/// ```
///
/// Behavior: one-shot responses queued via [`then_completed`](Self::then_completed) /
/// [`then_stopped`](Self::then_stopped) / [`then_error`](Self::then_error) are consumed
/// FIFO; once the queue is empty, the base behavior from the constructor answers. Every
/// run records the `ContextData` handle it was called with, so tests can inspect (or
/// mutate through) the exact contexts the code under test produced.
pub struct MockPipeline<TData, Err>
where
  TData: 'static + Send + Sync,
{
  base: BaseBehavior<TData, Err>,
  queued: Mutex<VecDeque<CannedResponse<Err>>>,
  seen_contexts: Mutex<Vec<ContextData<TData>>>,
}

impl<TData, Err> MockPipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Every run returns `Ok(PipelineResult::Completed)` (unless a queued response applies).
  pub fn completed() -> Self {
    Self::from_fn(|_ctx| Ok(PipelineResult::Completed))
  }

  /// Every run returns `Ok(PipelineResult::Stopped)` (unless a queued response applies).
  pub fn stopped() -> Self {
    Self::from_fn(|_ctx| Ok(PipelineResult::Stopped))
  }

  /// Every run fails with `make_err()` (unless a queued response applies).
  pub fn failing(make_err: impl Fn() -> Err + Send + Sync + 'static) -> Self {
    Self::from_fn(move |_ctx| Err(make_err()))
  }

  /// Full control: inspect the context, mutate it, decide the result.
  pub fn from_fn(f: impl Fn(ContextData<TData>) -> Result<PipelineResult, Err> + Send + Sync + 'static) -> Self {
    Self {
      base: Box::new(f),
      queued: Mutex::new(VecDeque::new()),
      seen_contexts: Mutex::new(Vec::new()),
    }
  }

  /// Queues a one-shot `Ok(Completed)` response, consumed before the base behavior.
  pub fn then_completed(&mut self) -> &mut Self {
    self.queued.lock().push_back(CannedResponse::Completed);
    self
  }

  /// Queues a one-shot `Ok(Stopped)` response, consumed before the base behavior.
  pub fn then_stopped(&mut self) -> &mut Self {
    self.queued.lock().push_back(CannedResponse::Stopped);
    self
  }

  /// Queues a one-shot error response, consumed before the base behavior.
  pub fn then_error(&mut self, make_err: impl Fn() -> Err + Send + Sync + 'static) -> &mut Self {
    self.queued.lock().push_back(CannedResponse::Error(Box::new(make_err)));
    self
  }

  /// How many times this mock has been run.
  pub fn run_count(&self) -> usize {
    self.seen_contexts.lock().len()
  }

  /// The `ContextData` handle from each run, in order. These are `Arc` handles, so a test
  /// can read the state each run saw or left behind.
  pub fn contexts(&self) -> Vec<ContextData<TData>> {
    self.seen_contexts.lock().clone()
  }
}

#[async_trait]
impl<TData, Err> PipelineRunner<TData, Err> for MockPipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    self.seen_contexts.lock().push(ctx_data.clone());
    let queued = self.queued.lock().pop_front();
    match queued {
      Some(CannedResponse::Completed) => Ok(PipelineResult::Completed),
      Some(CannedResponse::Stopped) => Ok(PipelineResult::Stopped),
      Some(CannedResponse::Error(make_err)) => Err(make_err()),
      None => (self.base)(ctx_data),
    }
  }
}


/// Asserts that the trace's completed steps are exactly `expected`, in order.
#[track_caller]
pub fn assert_steps_completed(trace: &TraceCollector, expected: &[&str]) {
  let completed = trace.completed_steps();
  if completed != expected {
    panic!(
      "completed steps mismatch\n  expected: {:?}\n  actual:   {:?}\n  all events:\n{}",
      expected,
      completed,
      format_events(trace)
    );
  }
}

/// Asserts that the trace's skipped steps are exactly `expected`, in order.
#[track_caller]
pub fn assert_steps_skipped(trace: &TraceCollector, expected: &[&str]) {
  let skipped = trace.skipped_steps();
  if skipped != expected {
    panic!(
      "skipped steps mismatch\n  expected: {:?}\n  actual:   {:?}\n  all events:\n{}",
      expected,
      skipped,
      format_events(trace)
    );
  }
}

/// Asserts the outcome of the most recently finished full run.
#[track_caller]
pub fn assert_run_outcome(trace: &TraceCollector, expected: RunOutcome) {
  let actual = trace.last_outcome();
  if actual.as_ref() != Some(&expected) {
    panic!(
      "run outcome mismatch\n  expected: {:?}\n  actual:   {:?}\n  all events:\n{}",
      expected,
      actual,
      format_events(trace)
    );
  }
}

/// Asserts that `expected` is an in-order subsequence of the trace's completed steps
/// (other steps may complete in between).
#[track_caller]
pub fn assert_order(trace: &TraceCollector, expected: &[&str]) {
  let completed = trace.completed_steps();
  let mut remaining = completed.iter();
  for want in expected {
    if !remaining.any(|s| s == want) {
      panic!(
        "expected {:?} as an in-order subsequence of completed steps, but '{}' was not found in order\n  completed: {:?}",
        expected, want, completed
      );
    }
  }
}

fn format_events(trace: &TraceCollector) -> String {
  trace
    .events()
    .iter()
    .map(|e| format!("    {}", e))
    .collect::<Vec<_>>()
    .join("\n")
}
