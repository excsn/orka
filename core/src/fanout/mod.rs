//! Fan-out: running one pipeline over every item of a runtime collection.
//!
//! [`conditional_scopes_for_step`](crate::Pipeline::conditional_scopes_for_step) is
//! one-of-N: the first scope whose condition matches runs. This is the all-of-N
//! counterpart, and it is a combinator rather than a step builder: call it from inside an
//! ordinary handler, get every item's outcome back, and decide what that means.
//!
//! ```ignore
//! let results = FanOut::new(target_pipeline.clone())
//!   .max_concurrent(8)
//!   .policy(FanOutPolicy::CollectAll)
//!   .run(ctx.with_ref(|c| c.placements.clone()))
//!   .await;
//!
//! ctx.with_mut(|c| c.deployed = results.cloned_oks());
//! results.into_control()
//! ```

pub(crate) mod joiner;
pub mod spawner;

use crate::core::context_data::ContextData;
use crate::core::control::{PipelineControl, PipelineResult};
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline;
use joiner::{BoundedJoin, BranchFuture, StopPredicate};
use parking_lot::Mutex;
use spawner::TaskSpawner;
use std::fmt;
use std::sync::Arc;

/// How a fan-out decides whether it succeeded.
///
/// [`FailFast`](Self::FailFast) is the only policy that can act before every branch has
/// settled, because it is the only one whose verdict is knowable early. It stops *starting*
/// new branches and lets in-flight ones finish rather than cancelling them: a cancelled
/// branch would be a dropped mid-run pipeline, whose
/// [`on_finish`](crate::Pipeline::on_finish) handlers would never fire and whose resource
/// bag would release late.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanOutPolicy {
  /// Stop starting new branches after the first failure, drain the in-flight ones, and
  /// report unsatisfied.
  FailFast,
  /// Run everything and always report satisfied. Failures are still in the results.
  CollectAll,
  /// Satisfied only if every branch ran without error.
  RequireAll,
  /// Satisfied if at least this many branches ran without error.
  RequireAtLeast(usize),
}

impl fmt::Display for FanOutPolicy {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      FanOutPolicy::FailFast => write!(f, "FailFast"),
      FanOutPolicy::CollectAll => write!(f, "CollectAll"),
      FanOutPolicy::RequireAll => write!(f, "RequireAll"),
      FanOutPolicy::RequireAtLeast(n) => write!(f, "RequireAtLeast({})", n),
    }
  }
}

type CustomPolicy<SData, Err> = Arc<dyn Fn(&FanOutResults<SData, Err>) -> bool + Send + Sync>;

enum Verdict<SData, Err>
where
  SData: Send + Sync + 'static,
{
  Builtin(FanOutPolicy),
  Custom(CustomPolicy<SData, Err>),
}

/// How one branch ended.
pub enum FanOutItemOutcome<Err> {
  /// The branch ran without error. Carries whether its pipeline completed or was stopped
  /// by one of its own handlers.
  Completed(PipelineResult),
  /// The branch failed. The error is the branch's own typed `Err`, not a string: each is
  /// produced once and owned, so nothing is lost aggregating them.
  Failed(Err),
  /// The branch never started, because [`FanOutPolicy::FailFast`] tripped first. It has
  /// run no code at all, and its context still holds the untouched input.
  NotStarted,
}

impl<Err> FanOutItemOutcome<Err> {
  pub fn is_success(&self) -> bool {
    matches!(self, FanOutItemOutcome::Completed(_))
  }

  pub fn is_failure(&self) -> bool {
    matches!(self, FanOutItemOutcome::Failed(_))
  }

  pub fn is_not_started(&self) -> bool {
    matches!(self, FanOutItemOutcome::NotStarted)
  }
}

impl<Err: fmt::Debug> fmt::Debug for FanOutItemOutcome<Err> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      FanOutItemOutcome::Completed(r) => f.debug_tuple("Completed").field(r).finish(),
      FanOutItemOutcome::Failed(e) => f.debug_tuple("Failed").field(e).finish(),
      FanOutItemOutcome::NotStarted => write!(f, "NotStarted"),
    }
  }
}

/// One branch's outcome, paired with the context it ran against.
pub struct FanOutItem<SData, Err>
where
  SData: Send + Sync + 'static,
{
  /// Position in the input collection. Items are always returned in input order.
  pub index: usize,
  /// The branch's own context. Readable afterwards for whatever the branch left in it.
  pub context: ContextData<SData>,
  pub outcome: FanOutItemOutcome<Err>,
}

/// Every branch's outcome, in input order.
///
/// A fan-out never discards results, including when its policy is unsatisfied: partial
/// success is data, not an error condition, so "three of five deployed" is answerable.
pub struct FanOutResults<SData, Err>
where
  SData: Send + Sync + 'static,
{
  items: Vec<FanOutItem<SData, Err>>,
  policy: FanOutPolicy,
  satisfied: bool,
}

impl<SData, Err> FanOutResults<SData, Err>
where
  SData: Send + Sync + 'static,
{
  pub fn items(&self) -> &[FanOutItem<SData, Err>] {
    &self.items
  }

  pub fn len(&self) -> usize {
    self.items.len()
  }

  pub fn is_empty(&self) -> bool {
    self.items.is_empty()
  }

  /// Branches that ran without error, whether their pipeline completed or stopped.
  pub fn succeeded(&self) -> usize {
    self.items.iter().filter(|i| i.outcome.is_success()).count()
  }

  pub fn failed(&self) -> usize {
    self.items.iter().filter(|i| i.outcome.is_failure()).count()
  }

  /// Branches that ran without error but whose pipeline was stopped by a handler. These
  /// are a subset of [`succeeded`](Self::succeeded).
  pub fn stopped(&self) -> usize {
    self
      .items
      .iter()
      .filter(|i| matches!(i.outcome, FanOutItemOutcome::Completed(PipelineResult::Stopped)))
      .count()
  }

  pub fn not_started(&self) -> usize {
    self.items.iter().filter(|i| i.outcome.is_not_started()).count()
  }

  /// Contexts of the branches that ran without error.
  pub fn oks(&self) -> impl Iterator<Item = &ContextData<SData>> {
    self
      .items
      .iter()
      .filter(|i| i.outcome.is_success())
      .map(|i| &i.context)
  }

  /// Snapshots of the successful branches' contexts, in input order.
  pub fn cloned_oks(&self) -> Vec<SData>
  where
    SData: Clone,
  {
    self.oks().map(|c| c.with_ref(|s| s.clone())).collect()
  }

  /// Each failure with its input index, so a caller can say *which* item failed rather
  /// than only how many did.
  pub fn errors(&self) -> impl Iterator<Item = (usize, &Err)> {
    self.items.iter().filter_map(|i| match &i.outcome {
      FanOutItemOutcome::Failed(e) => Some((i.index, e)),
      _ => None,
    })
  }

  pub fn policy(&self) -> &FanOutPolicy {
    &self.policy
  }

  /// Whether the configured policy was met.
  pub fn satisfied(&self) -> bool {
    self.satisfied
  }

  /// The first branch failure, in input order. Consumes the results, since `Err` is not
  /// required to be `Clone`.
  pub fn into_first_error(self) -> Option<Err> {
    self.items.into_iter().find_map(|i| match i.outcome {
      FanOutItemOutcome::Failed(e) => Some(e),
      _ => None,
    })
  }

  /// The handler-body convenience: `Ok(Continue)` when the policy was satisfied, otherwise
  /// the first branch's typed error, or [`OrkaError::FanOutPolicyUnmet`] when the policy
  /// was unmet without any branch failing (`RequireAll` over branches that all stopped).
  ///
  /// Consumes the results, so read anything you need out of them first.
  pub fn into_control(self) -> Result<PipelineControl, Err>
  where
    Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  {
    if self.satisfied {
      return Ok(PipelineControl::Continue);
    }
    let unmet = OrkaError::FanOutPolicyUnmet {
      policy: self.policy.to_string(),
      total: self.len(),
      succeeded: self.succeeded(),
      failed: self.failed(),
      not_started: self.not_started(),
    };
    Err(self.into_first_error().unwrap_or_else(|| Err::from(unmet)))
  }
}

impl<SData, Err> fmt::Display for FanOutResults<SData, Err>
where
  SData: Send + Sync + 'static,
{
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} item(s): {} succeeded, {} failed, {} not started ({}: {})",
      self.len(),
      self.succeeded(),
      self.failed(),
      self.not_started(),
      self.policy,
      if self.satisfied { "satisfied" } else { "unmet" }
    )
  }
}

/// Runs one pipeline over every item of a collection, with bounded concurrency.
///
/// Configure once, run many times. Each branch is a full
/// [`Pipeline::run`](crate::Pipeline::run), so every item gets its own `on_finish` ring,
/// its own resource bag release, and its own run id in any trace.
///
/// Concurrency here is cooperative: branches are polled on the caller's task, making
/// progress while each other awaits. That suits I/O-bound work; a branch that blocks the
/// thread stalls its siblings.
pub struct FanOut<SData, Err>
where
  SData: Send + Sync + 'static,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pipeline: Arc<Pipeline<SData, Err>>,
  verdict: Verdict<SData, Err>,
  max_concurrent: usize,
  spawner: Option<Arc<dyn TaskSpawner>>,
}

impl<SData, Err> FanOut<SData, Err>
where
  SData: Send + Sync + 'static,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Defaults to [`FanOutPolicy::CollectAll`] and unbounded concurrency.
  pub fn new(pipeline: Arc<Pipeline<SData, Err>>) -> Self {
    Self {
      pipeline,
      verdict: Verdict::Builtin(FanOutPolicy::CollectAll),
      max_concurrent: usize::MAX,
      spawner: None,
    }
  }

  /// Runs each branch as a task on your executor instead of cooperatively on the caller's
  /// task, which is what turns concurrency into parallelism. See [`TaskSpawner`] for the
  /// (short) implementation and for the two semantics that change: a panicking branch
  /// becomes a contained failure rather than unwinding the fan-out, and dropping the
  /// fan-out no longer cancels in-flight branches.
  ///
  /// [`max_concurrent`](Self::max_concurrent) and fail-fast still govern *starting*,
  /// because a branch spawns only when the fan-out first polls it.
  pub fn spawner(mut self, spawner: Arc<dyn TaskSpawner>) -> Self {
    self.spawner = Some(spawner);
    self
  }

  pub fn policy(mut self, policy: FanOutPolicy) -> Self {
    self.verdict = Verdict::Builtin(policy);
    self
  }

  /// A policy the four built-ins cannot express ("satisfied if the primary region
  /// succeeded"). Replaces any policy set by [`policy`](Self::policy).
  ///
  /// A custom policy is evaluated once, after every branch has settled, so unlike
  /// [`FanOutPolicy::FailFast`] it cannot stop branches from starting.
  pub fn custom_policy(mut self, is_satisfied: impl Fn(&FanOutResults<SData, Err>) -> bool + Send + Sync + 'static) -> Self {
    self.verdict = Verdict::Custom(Arc::new(is_satisfied));
    self
  }

  /// Caps how many branches are in flight at once. Unbounded by default.
  ///
  /// # Panics
  /// Panics if `n` is zero.
  pub fn max_concurrent(mut self, n: usize) -> Self {
    assert!(n > 0, "Orka setup error: max_concurrent must be at least 1.");
    self.max_concurrent = n;
    self
  }

  /// Runs the pipeline once per item and returns every outcome, in input order.
  ///
  /// Never discards results, including when the policy is unmet: see
  /// [`FanOutResults::into_control`] for turning the verdict into a handler's return.
  pub async fn run<I>(&self, items: I) -> FanOutResults<SData, Err>
  where
    I: IntoIterator<Item = SData>,
  {
    let contexts: Vec<ContextData<SData>> = items.into_iter().map(ContextData::new).collect();

    let branches: Vec<BranchFuture<Result<PipelineResult, Err>>> = contexts
      .iter()
      .enumerate()
      .map(|(index, ctx)| {
        // Cloning the Arc and the context handle makes each branch 'static.
        let pipeline = self.pipeline.clone();
        let ctx = ctx.clone();

        match self.spawner.clone() {
          None => Box::pin(async move { pipeline.run(ctx).await }) as BranchFuture<_>,
          Some(spawner) => Box::pin(async move {
            // Everything here runs on this branch's *first poll*, which is exactly when
            // the joiner decides to start it. That laziness is what keeps max_concurrent
            // and fail-fast governing spawn timing without the joiner knowing about
            // spawning at all.
            let slot: Arc<Mutex<Option<Result<PipelineResult, Err>>>> = Arc::new(Mutex::new(None));
            let write_slot = slot.clone();

            let handle = spawner.spawn(Box::pin(async move {
              let outcome = pipeline.run(ctx).await;
              *write_slot.lock() = Some(outcome);
            }));
            handle.await;

            // An empty slot means the task never finished normally: the runtime caught a
            // panic, or the task was aborted. Report it as this branch's failure instead
            // of letting it masquerade as success.
            let outcome = slot.lock().take();
            outcome.unwrap_or_else(|| Err(Err::from(OrkaError::FanOutBranchLost { index })))
          }) as BranchFuture<_>,
        }
      })
      .collect();

    let fail_fast = matches!(self.verdict, Verdict::Builtin(FanOutPolicy::FailFast));
    let stop_on: Option<StopPredicate<Result<PipelineResult, Err>>> =
      fail_fast.then(|| Box::new(|r: &Result<PipelineResult, Err>| r.is_err()) as StopPredicate<_>);

    let outcomes = BoundedJoin::new(branches, self.max_concurrent, stop_on).await;

    let items: Vec<FanOutItem<SData, Err>> = contexts
      .into_iter()
      .zip(outcomes)
      .enumerate()
      .map(|(index, (context, outcome))| {
        let outcome = match outcome {
          Some(Ok(result)) => FanOutItemOutcome::Completed(result),
          Some(Err(e)) => FanOutItemOutcome::Failed(e),
          None => FanOutItemOutcome::NotStarted,
        };
        FanOutItem { index, context, outcome }
      })
      .collect();

    let policy = match &self.verdict {
      Verdict::Builtin(p) => p.clone(),
      // A custom policy still reports a name in FanOutPolicyUnmet; CollectAll is the
      // closest built-in shape (run everything, decide at the end).
      Verdict::Custom(_) => FanOutPolicy::CollectAll,
    };

    let mut results = FanOutResults {
      items,
      policy,
      satisfied: false,
    };

    results.satisfied = match &self.verdict {
      Verdict::Builtin(FanOutPolicy::CollectAll) => true,
      Verdict::Builtin(FanOutPolicy::FailFast) => results.failed() == 0,
      Verdict::Builtin(FanOutPolicy::RequireAll) => results.succeeded() == results.len(),
      Verdict::Builtin(FanOutPolicy::RequireAtLeast(n)) => results.succeeded() >= *n,
      Verdict::Custom(is_satisfied) => is_satisfied(&results),
    };

    results
  }
}
