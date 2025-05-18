// orka/src/core/control.rs

//! Defines signals for controlling pipeline flow and the outcome of a pipeline run.

/// Signal from a handler indicating whether the pipeline should continue or stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineControl {
  /// Continue processing the current step and subsequent steps.
  Continue,
  /// Stop processing the current step immediately and halt the pipeline.
  /// No further handlers in the current step or subsequent steps will be executed.
  Stop,
  // Potential future addition:
  // /// Skip the remaining handlers (on, after) for the current step, but continue to the next step.
  // SkipCurrentStepRest,
}

/// Outcome of a full pipeline execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineResult {
  /// The pipeline executed all its non-skipped, non-optional steps to completion.
  Completed,
  /// The pipeline was explicitly stopped by a handler returning `PipelineControl::Stop`.
  Stopped,
}
