// src/lib.rs

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
pub use crate::core::step::StepDef;
pub use crate::core::context::{Handler, ContextDataExtractorImpl};
pub use crate::core::context_data::ContextData;

// The main Pipeline struct and its primary builder for conditional logic
pub use crate::pipeline::definition::Pipeline;
// The builder for conditional scopes is a key part of the fluent API
pub use crate::conditional::builder::ConditionalScopeBuilder;
pub use crate::conditional::builder::ConditionalScopeConfigurator;

pub use crate::conditional::provider::{PipelineProvider, StaticPipelineProvider};

pub use crate::error::{OrkaError, OrkaResult};

// The Orka registry for managing and dispatching pipelines
pub use crate::registry::Orka;

// --- General Crate-Level Items ---

// Standard Result type used throughout Orka, typically wrapping anyhow::Error
// pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;
// However, individual modules/functions will likely define their own Result<T> = anyhow::Result<T>
// or Result<T, SpecificError>. `anyhow::Result` is often used directly.

// Greet the user (optional, for fun)
// pub fn greet() {
//     println!("Welcome to Orka Workflow Engine!");
// }

// Example of a high-level comment explaining a core concept if needed.
/*
    Core Workflow:
    1. Define a context struct `MyCtx` for your process.
    2. Create a `Pipeline<MyCtx>` instance, defining its steps.
    3. Register asynchronous handlers for these steps using `.on_root()`, `.before_root()`, etc.
    4. For complex conditional logic within a step:
       - Use `pipeline.conditional_scopes_for_step("step_name")`
       - Chain `.add_dynamic_scope()` or `.add_static_scope()`, providing:
         - A way to get a scoped `Pipeline<ScopedCtx>` (either an Arc or a factory function).
         - An extractor `Fn(&mut MyCtx) -> Result<&mut ScopedCtx>`.
       - Chain `.on_condition()` with `Fn(&MyCtx) -> bool`.
       - Call `.finalize_conditional_step()` (optionally with `.if_no_scope_matches()`).
    5. Create an `Orka` registry instance.
    6. Register your `Pipeline<MyCtx>` with the `Orka` instance.
    7. When ready to execute, create an instance of `MyCtx`, and call `orka_instance.run(&mut my_ctx_instance).await`.
*/

// Ensure all public items are documented.
// Consider using `#![warn(missing_docs)]` at the crate level once stable.