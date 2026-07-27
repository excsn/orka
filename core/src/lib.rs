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

pub mod core;
pub mod pipeline;
pub mod conditional;
pub mod fanout;
pub mod registry;
pub mod error;
#[cfg(feature = "tokio")]
pub mod time;

pub use crate::core::control::{PipelineControl, PipelineResult};
pub use crate::core::step::{SkipCondition, StepDef};
pub use crate::core::context::{AnyContextDataExtractor, ContextDataExtractorImpl, FinishHandler, Handler};
pub use crate::core::context_data::ContextData;
pub use crate::core::resources::RunResources;

pub use crate::core::trace::{
  CompositeObserver, HandlerOutcome, PipelineObserver, RunOutcome, RunTrace, SkipReason, StepPhase, TraceCollector,
  TraceEvent, TraceEventKind,
};

pub use crate::pipeline::definition::Pipeline;
pub use crate::pipeline::execution::{PlannedAction, StepPlan};
pub use crate::pipeline::runner::PipelineRunner;
pub use crate::conditional::builder::ConditionalScopeBuilder;
pub use crate::conditional::builder::ConditionalScopeConfigurator;

pub use crate::conditional::provider::{
  DynPipelineProvider, FunctionalPipelineProvider, PipelineProvider, StaticPipelineProvider,
};

pub use crate::fanout::spawner::{SpawnHandle, SpawnedTask, TaskSpawner};
#[cfg(feature = "tokio")]
pub use crate::fanout::spawner::TokioSpawner;
pub use crate::fanout::{FanOut, FanOutItem, FanOutItemOutcome, FanOutPolicy, FanOutResults};

#[cfg(feature = "tokio")]
pub use crate::time::timed;

pub use crate::error::{OrkaError, OrkaResult};

pub use crate::registry::Orka;

#[cfg(feature = "test-util")]
pub mod test_util;

/// The common imports for building and running a pipeline.
///
/// ```ignore
/// use orka::prelude::*;
/// ```
///
/// The rule: a type is here if you cannot avoid naming it to use the everyday
/// build/configure/run/inspect surface. That covers the parameter and return types of
/// `Pipeline`'s own methods: [`RunOutcome`] for [`on_finish`](Pipeline::on_finish) and
/// [`run_with_outcome`](Pipeline::run_with_outcome), [`StepPlan`] / [`PlannedAction`] /
/// [`SkipReason`] for [`resolve_plan`](Pipeline::resolve_plan), and [`StepPhase`] for
/// [`has_handlers`](Pipeline::has_handlers).
///
/// Two clusters are deliberately left out, because you reach for them on purpose rather
/// than meeting them in a signature: the **advanced** surface (`Handler`,
/// `ContextDataExtractorImpl`, `AnyContextDataExtractor`, the pipeline providers, the
/// conditional-scope builders) and the **observability** surface (`TraceCollector`,
/// `PipelineObserver`, `CompositeObserver`, `TraceEvent`, `TraceEventKind`,
/// `HandlerOutcome`, `RunTrace`). Import those from the crate root.
pub mod prelude {
  pub use crate::core::context_data::ContextData;
  pub use crate::core::control::{PipelineControl, PipelineResult};
  pub use crate::core::step::{SkipCondition, StepDef};
  pub use crate::core::trace::{RunOutcome, SkipReason, StepPhase};
  pub use crate::error::{OrkaError, OrkaResult};
  pub use crate::pipeline::definition::Pipeline;
  pub use crate::pipeline::execution::{PlannedAction, StepPlan};
  pub use crate::pipeline::runner::PipelineRunner;
  pub use crate::registry::Orka;
}