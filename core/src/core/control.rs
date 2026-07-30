//! Defines signals for controlling pipeline flow and the outcome of a pipeline run.

/// Signal from a handler indicating whether the pipeline should continue or stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineControl {
  /// Continue processing the current step and subsequent steps.
  Continue,
  /// Stop processing the current step immediately and halt the pipeline.
  /// No further handlers in the current step or subsequent steps will be executed.
  Stop,
}

/// Outcome of a full pipeline execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipelineResult {
  /// The pipeline executed all its non-skipped, non-optional steps to completion.
  Completed,
  /// The pipeline was explicitly stopped by a handler returning `PipelineControl::Stop`.
  Stopped,
  /// The pipeline reached a step boundary with its
  /// [`CancelToken`](crate::CancelToken) set and wound down without starting that step.
  ///
  /// Distinct from [`Stopped`](Self::Stopped) because the two want opposite cleanup: a
  /// stop is a deliberate early exit, a cancellation leaves whatever the run had half
  /// built. A handler returning `Stop` while the token is set reports as this, since the
  /// token's verdict outranks the handler's.
  Cancelled,
}
