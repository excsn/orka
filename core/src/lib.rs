//! Orka: An ASYNC pluggable, type-safe workflow engine for Rust.
//!
//! Orka allows you to define complex, multi-step processes (pipelines)
//! with features like:
//!  - Named steps with before/on/after hooks.
//!  - Asynchronous handlers for I/O-bound operations.
//!  - Early stopping or continuing of pipeline execution.
//!  - Dynamic step mutation (inserting, removing steps).
//!  - Per-step extractors for operating on sub-contexts.
//!  - Conditional execution of scoped pipelines, allowing dynamic workflow branching.
//!  - A type-keyed registry for managing and running different pipelines.

// Declare modules according to the planned structure
pub mod core;
pub mod pipeline;
pub mod conditional;
pub mod registry;
pub mod error;

// --- Re-exports for the Public API ---

// Core types that users will interact with frequently
pub use crate::core::control::{PipelineControl, PipelineResult};
pub use crate::core::step::{SkipCondition, StepDef};
pub use crate::core::context::{Handler, ContextDataExtractorImpl};
pub use crate::core::context_data::ContextData;

// The main Pipeline struct and its primary builder for conditional logic
pub use crate::pipeline::definition::Pipeline;
// The builder for conditional scopes is a key part of the fluent API
pub use crate::conditional::builder::ConditionalScopeBuilder;
pub use crate::conditional::builder::ConditionalScopeConfigurator;

pub use crate::conditional::provider::{FunctionalPipelineProvider, PipelineProvider, StaticPipelineProvider};

pub use crate::error::{OrkaError, OrkaResult};

// The Orka registry for managing and dispatching pipelines
pub use crate::registry::Orka;

/// The common imports for building and running a pipeline.
///
/// ```ignore
/// use orka::prelude::*;
/// ```
///
/// Advanced surface — `Handler`, `ContextDataExtractorImpl`, the pipeline providers, and the
/// conditional-scope builders — is deliberately left out; import those from the crate root
/// when you need them.
pub mod prelude {
  pub use crate::core::context_data::ContextData;
  pub use crate::core::control::{PipelineControl, PipelineResult};
  pub use crate::core::step::{SkipCondition, StepDef};
  pub use crate::error::{OrkaError, OrkaResult};
  pub use crate::pipeline::definition::Pipeline;
  pub use crate::registry::Orka;
}