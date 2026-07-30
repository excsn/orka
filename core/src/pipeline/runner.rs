//! The [`PipelineRunner`] trait: the object-safe composition seam at a pipeline's run
//! boundary.

use crate::core::context_data::ContextData;
use crate::core::control::PipelineResult;
use crate::core::trace::RunOutcome;
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline;
use async_trait::async_trait;

/// Object-safe run boundary for a pipeline.
///
/// `Pipeline<TData, Err>` implements this, so anything that only needs to *execute* a
/// pipeline can hold an `Arc<dyn PipelineRunner<TData, Err>>` instead of the concrete
/// type. That one indirection is both a test seam and a production seam:
///
/// - In tests, hand the holder a mock (see `orka::test_util::MockPipeline` with the
///   `test-util` feature) that returns canned `Completed`/`Stopped`/`Err` results, or
///   register one into an [`Orka`](crate::Orka) registry via
///   [`register_runner`](crate::Orka::register_runner).
/// - In production, wrap another runner to compose middleware: retry, timeout, logging,
///   metrics; each is just an implementation that delegates to an inner
///   `Arc<dyn PipelineRunner>`.
#[async_trait]
pub trait PipelineRunner<TData, Err>: Send + Sync
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>;

  /// As [`run`](Self::run), additionally returning the [`RunOutcome`].
  ///
  /// The default implementation derives the outcome from `run()`'s result; on failure the
  /// outcome's `step` is empty, meaning "unknown: this runner cannot attribute failures
  /// to a step". `Pipeline` overrides this with real step attribution; middleware runners
  /// should delegate to their inner runner's `run_with_outcome` to preserve it.
  async fn run_with_outcome(&self, ctx_data: ContextData<TData>) -> (Result<PipelineResult, Err>, RunOutcome) {
    let result = self.run(ctx_data).await;
    let outcome = match &result {
      Ok(PipelineResult::Completed) => RunOutcome::Completed,
      Ok(PipelineResult::Stopped) => RunOutcome::Stopped,
      Ok(PipelineResult::Cancelled) => RunOutcome::Cancelled,
      Err(e) => RunOutcome::Errored {
        step: String::new(),
        message: e.to_string(),
      },
    };
    (result, outcome)
  }
}

#[async_trait]
impl<TData, Err> PipelineRunner<TData, Err> for Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    // The inherent method keeps precedence for direct callers; this impl only exists so
    // a Pipeline can be used wherever a runner is expected.
    Pipeline::run(self, ctx_data).await
  }

  async fn run_with_outcome(&self, ctx_data: ContextData<TData>) -> (Result<PipelineResult, Err>, RunOutcome) {
    // Real step attribution, unlike the trait default.
    Pipeline::run_with_outcome(self, ctx_data).await
  }
}
