//! Bounding a single await inside a handler, with the failure reported as a step timeout.

use crate::error::OrkaError;
use std::future::Future;
use std::time::Duration;

/// Awaits `fut` with a time budget, reporting an overrun as
/// [`OrkaError::StepTimedOut`] rather than an anonymous elapsed error.
///
/// Orka imposes no timeouts of its own: it depends on no runtime, so it has no timer and
/// cannot bound a handler for you. This is the small piece it can offer, collapsing the
/// match-and-map every hand-rolled timeout otherwise repeats, and making sure the failure
/// names the step. That matters because [`Pipeline::run`](crate::Pipeline::run) discards
/// the [`RunOutcome`](crate::RunOutcome) that would otherwise carry the attribution, and a
/// fan-out branch keeps only its typed error.
///
/// The budget bounds **this await only**, not the rest of the handler. Reach for it when a
/// specific call may never return, which is the usual shape: waiting on a remote push, a
/// channel that may go quiet, a socket read.
///
/// ```ignore
/// pipeline.on_root(Step::AwaitArtifact, |ctx| async move {
///   let (rx, budget) = ctx.with_ref(|c| (c.archive_ready_rx.clone(), c.artifact_timeout));
///
///   // Two independent failure modes, so two unwraps: the timeout, then the receive.
///   let msg = timed(Step::AwaitArtifact, budget, rx.recv()).await??;
///
///   ctx.with_mut(|c| c.artifact_id = msg.artifact_id);
///   Ok(PipelineControl::Continue)
/// });
/// ```
///
/// The returned `OrkaError` converts into the pipeline's own error type through the
/// `From<OrkaError>` bound every pipeline error carries, so a single `?` discharges it.
///
/// On expiry `fut` is dropped, abandoning whatever it had in flight. The run itself
/// continues to its exit, so its [`on_finish`](crate::Pipeline::on_finish) ring still runs
/// and its [`resources`](crate::ContextData::resources) bag still releases at the usual
/// point. Only state the future held locally is lost, which is a reason to stash anything
/// needing orderly shutdown (a stream sender, a lock guard) in the resource bag rather
/// than in a local.
pub async fn timed<T, F>(step_name: impl AsRef<str>, budget: Duration, fut: F) -> Result<T, OrkaError>
where
  F: Future<Output = T>,
{
  match tokio::time::timeout(budget, fut).await {
    Ok(value) => Ok(value),
    Err(_) => Err(OrkaError::StepTimedOut {
      step_name: step_name.as_ref().to_string(),
      after: budget,
    }),
  }
}
