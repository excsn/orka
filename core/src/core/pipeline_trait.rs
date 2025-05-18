// orka/src/core/pipeline_trait.rs

//! Defines the `AnyPipeline<E>` trait for type-erased pipeline execution by the Orka registry.

use crate::core::control::PipelineResult;
use crate::error::OrkaError;
use async_trait::async_trait;
use std::any::Any; // Orka's specific error type

/// A type-erased trait that allows the `Orka` registry to run pipelines
/// without knowing their specific context type `T` at the registry level.
///
/// It is generic over `E`, the error type that the application wants `Orka::run` to return.
/// `E` must be creatable from `OrkaError`.
#[async_trait]
pub trait AnyPipeline<E>: Send + Sync
where
  E: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Executes the pipeline with a type-erased context (`dyn Any`).
  /// Implementations must attempt to downcast `ctx` to their specific context type.
  /// Returns `Ok(PipelineResult)` on successful execution (Completed or Stopped),
  /// or `Err(E)` if an error occurs.
  async fn run_any_erased(&self, ctx: &mut dyn Any) -> Result<PipelineResult, E>;
}
