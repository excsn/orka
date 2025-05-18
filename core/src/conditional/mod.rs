// orka/src/conditional/mod.rs

//! Enables conditional execution of scoped pipelines within a step of a main pipeline.
//!
//! This module provides the `ConditionalScopeBuilder` for a fluent API to define
//! different execution paths (scopes) based on conditions evaluated against the
//! main pipeline's context. Each scope can dynamically source or use a static
//! `Pipeline<S>` instance and operate on an extracted sub-context `S`.

pub mod builder;
pub mod provider;
pub mod scope;

// Re-export the primary builder for users.
// Other types like PipelineProvider might be used by advanced users
// or for testing, but are often an implementation detail of the builder.
pub use builder::ConditionalScopeBuilder;
// pub use provider::PipelineProvider; // Optional re-export