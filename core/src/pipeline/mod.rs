//! Defines the `Pipeline<T>` struct, its construction, modification, and execution logic.

pub mod definition;
pub mod execution;
pub mod hooks;
pub mod runner;

pub use definition::Pipeline;
pub use execution::{PlannedAction, StepPlan};
pub use runner::PipelineRunner;