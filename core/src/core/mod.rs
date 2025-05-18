pub mod context;
pub mod context_data;
pub mod control;
pub mod pipeline_trait;
pub mod step;
// Optional: pub mod error; // If OrkaError is defined here instead of top-level src/error.rs

// Re-export key types for easier access from other Orka modules (and potentially lib.rs)
pub use context::Handler; // The Handler<T> type alias
pub use context_data::ContextData; // The Handler<T> type alias
pub use control::{PipelineControl, PipelineResult};
pub use pipeline_trait::AnyPipeline;
pub use step::StepDef;
