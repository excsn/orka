use anyhow::Error as AnyhowError;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrkaError {
    #[error("Step not found: {step_name}")]
    StepNotFound { step_name: String },

    #[error("Handler missing for non-optional step: {step_name}")]
    HandlerMissing { step_name: String },

    #[error("Extractor failed for step '{step_name}'. Source: {source}")]
    ExtractorFailure {
        step_name: String,
        #[source]
        source: AnyhowError,
    },

    #[error("Pipeline provider failed for conditional scope in step '{step_name}'. Source: {source}")]
    PipelineProviderFailure {
        step_name: String,
        #[source]
        source: AnyhowError,
    },

    #[error("Type mismatch during context downcast (expected {expected_type}, step: '{step_name}')")]
    TypeMismatch {
        step_name: String,
        expected_type: String,
    },

    #[error("Error in user-provided handler or external operation. Source: {source}")]
    HandlerError {
        #[source]
        source: AnyhowError,
    },
    
    #[error("Configuration error for step '{step_name}': {message}")]
    ConfigurationError { step_name: String, message: String },

    #[error("Internal Orka error: {0}")]
    Internal(String),
    #[error("No conditional scope's condition matched for step '{step_name}'")]
    NoConditionalScopeMatched { step_name: String },

    /// A fan-out finished without satisfying its policy, and no branch produced an error
    /// to propagate (for example `RequireAll` where every branch merely stopped). When a
    /// branch *did* fail, that branch's own typed error is returned instead.
    #[error(
        "Fan-out policy {policy} unmet: {succeeded} of {total} succeeded ({failed} failed, {not_started} never started)"
    )]
    FanOutPolicyUnmet {
        policy: String,
        total: usize,
        succeeded: usize,
        failed: usize,
        not_started: usize,
    },

    /// A fan-out branch was started on a [`TaskSpawner`](crate::TaskSpawner) but never
    /// produced a result, meaning its task panicked or was aborted. Only reachable when a
    /// spawner is configured; cooperatively polled branches unwind into the caller instead.
    #[error("Fan-out branch {index} did not run to completion (its task panicked or was aborted)")]
    FanOutBranchLost { index: usize },

    /// A step read a resource that is not present in the context.
    ///
    /// Produced by [`ContextData::require`](crate::ContextData::require), which exists so
    /// that a missing resource is a handled error rather than the `.expect()` panic it
    /// would otherwise be. The distinction is not cosmetic: a panic unwinds past the run's
    /// [`on_finish`](crate::Pipeline::on_finish) ring and past
    /// [`RunResources`](crate::RunResources) release, so cleanup is skipped and what does
    /// drop, drops in the wrong order. Inside a spawned fan-out it degrades further, into
    /// [`FanOutBranchLost`](Self::FanOutBranchLost).
    ///
    /// The resource name is the one you passed; the *step* comes from
    /// [`Pipeline::run_with_outcome`](crate::Pipeline::run_with_outcome), since a context
    /// does not know which step is reading it.
    #[error("Required resource '{resource}' is not present in the context (was its producing step skipped or removed?)")]
    ResourceMissing { resource: String },

    /// A step exceeded its time budget.
    ///
    /// Orka does not impose timeouts: it depends on no async runtime, so it has no timer
    /// and cannot wake a hung handler. A timeout is therefore the handler's own business,
    /// wrapping its work in whatever its runtime provides. This variant exists so that
    /// every such handler reports the outcome the same way, and so the failure carries the
    /// step's name into [`Pipeline::run_with_outcome`](crate::Pipeline::run_with_outcome)
    /// and the trace instead of arriving as an anonymous error.
    ///
    /// ```ignore
    /// pipeline.on_root(Step::Install, |ctx| async move {
    ///   let budget = Duration::from_secs(300);
    ///   match tokio::time::timeout(budget, install(ctx)).await {
    ///     Ok(result) => result,
    ///     Err(_) => Err(OrkaError::StepTimedOut { step_name: "install".into(), after: budget })?,
    ///   }
    /// });
    /// ```
    ///
    /// Note the handler is dropped when its timeout fires, so any work it had in flight is
    /// abandoned wherever it reached. The run's [`on_finish`](crate::Pipeline::on_finish)
    /// ring and its resource bag still release normally, since the run itself continues to
    /// its exit.
    #[error("Step '{step_name}' timed out after {after:?}")]
    StepTimedOut { step_name: String, after: Duration },
}

// An `anyhow::Error` that already wraps an `OrkaError` is deliberately left nested rather
// than unwrapped: `OrkaError` is not `Clone`, and `HandlerError` keeps the original as a
// `#[source]`, so the causal chain stays intact either way.
impl From<AnyhowError> for OrkaError {
  fn from(err: AnyhowError) -> Self {
    OrkaError::HandlerError { source: err }
  }
}

pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;