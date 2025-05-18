// orka/src/conditional/provider.rs

//! Defines the `PipelineProvider` trait and its implementations for sourcing
//! `Arc<Pipeline<SData, MainErr>>` instances.
//! Operates with `ContextData<TData>`.

use crate::core::context_data::ContextData;
use crate::error::{OrkaError, OrkaResult}; // OrkaResult is Result<_, OrkaError>
use crate::pipeline::Pipeline; // Will be Pipeline<SData, MainErr>
use anyhow::Context as AnyhowContext; // For .with_context()
use async_trait::async_trait;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

/// A trait for objects that can provide an instance of an `Arc<Pipeline<SData, MainErr>>`.
///
/// `TData` is the main pipeline's underlying context data type.
/// `SData` is the scoped pipeline's underlying context data type.
/// `MainErr` is the error type used by both the main pipeline and the scoped pipeline.
/// It must be able to be created from `OrkaError` for framework-level issues during provider operation.
#[async_trait]
pub trait PipelineProvider<TData, SData, MainErr>: Send + Sync + 'static
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Asynchronously gets or creates an `Arc<Pipeline<SData, MainErr>>` instance.
  ///
  /// The `main_ctx_data: ContextData<TData>` is provided (cloned).
  /// The result is `Result<Arc<Pipeline<SData, MainErr>>, OrkaError>` because the
  /// *provider's own operation* (e.g., factory logic failing to initialize before pipeline creation)
  /// can fail with a framework-level `OrkaError`. This OrkaError will then be converted to `MainErr`
  /// by the calling `AnyConditionalScope` or `ConditionalScopeBuilder`.
  async fn get_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<Arc<Pipeline<SData, MainErr>>, OrkaError>;
}

// --- Static Pipeline Provider ---

/// Provides a pre-existing, static `Arc<Pipeline<SData, MainErr>>`.
#[derive(Clone)]
pub struct StaticPipelineProvider<SData, MainErr>
where
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pipeline: Arc<Pipeline<SData, MainErr>>,
  _phantom_sdata: PhantomData<SData>,
  _phantom_main_err: PhantomData<MainErr>,
}

impl<SData, MainErr> StaticPipelineProvider<SData, MainErr>
where
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pub fn new(pipeline: Arc<Pipeline<SData, MainErr>>) -> Self {
    Self {
      pipeline,
      _phantom_sdata: PhantomData,
      _phantom_main_err: PhantomData,
    }
  }
}

#[async_trait]
impl<TData, SData, MainErr> PipelineProvider<TData, SData, MainErr> for StaticPipelineProvider<SData, MainErr>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  async fn get_pipeline(&self, _main_ctx_data: ContextData<TData>) -> Result<Arc<Pipeline<SData, MainErr>>, OrkaError> {
    Ok(self.pipeline.clone())
  }
}

// --- Functional Pipeline Provider ---

/// Provides an `Arc<Pipeline<SData, MainErr>>` by invoking a user-supplied asynchronous factory function.
///
/// The factory function `F` takes `ContextData<TData>` (cloned) and returns a `Future`
/// that resolves to `Result<Arc<Pipeline<SData, MainErr>>, OrkaError>`.
pub struct FunctionalPipelineProvider<TData, SData, MainErr, F, Fut>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<Arc<Pipeline<SData, MainErr>>, OrkaError>> + Send + 'static,
{
  factory: F,
  _phantom_tdata: PhantomData<fn() -> TData>,
  _phantom_sdata: PhantomData<fn() -> SData>,
  _phantom_main_err: PhantomData<fn() -> MainErr>,
}

impl<TData, SData, MainErr, F, Fut> FunctionalPipelineProvider<TData, SData, MainErr, F, Fut>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<Arc<Pipeline<SData, MainErr>>, OrkaError>> + Send + 'static,
{
  pub fn new(factory: F) -> Self {
    Self {
      factory,
      _phantom_tdata: PhantomData,
      _phantom_sdata: PhantomData,
      _phantom_main_err: PhantomData,
    }
  }
}

#[async_trait]
impl<TData, SData, MainErr, F, Fut> PipelineProvider<TData, SData, MainErr>
  for FunctionalPipelineProvider<TData, SData, MainErr, F, Fut>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static,
  Fut: Future<Output = Result<Arc<Pipeline<SData, MainErr>>, OrkaError>> + Send + 'static,
{
  async fn get_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<Arc<Pipeline<SData, MainErr>>, OrkaError> {
    let sdata_type_name = std::any::type_name::<SData>(); // Capture for context message

    // The user's factory future already returns Result<_, OrkaError>.
    // If it's Ok, .with_context() does nothing.
    // If it's Err(OrkaError), .with_context() wraps it in anyhow::Error.
    // So, the .map_err must convert this anyhow::Error back to an OrkaError.
    (self.factory)(main_ctx_data).await.map_err(|orka_err_from_factory| {
      // The factory itself failed with an OrkaError.
      // We can enrich this OrkaError or return a new one.
      // Let's create a PipelineProviderFailure, potentially wrapping the original.
      OrkaError::PipelineProviderFailure {
        step_name: format!("functional_provider_for_{}", sdata_type_name),
        source: anyhow::anyhow!(
          "Factory for SData='{}' failed: {}", // Context message
          sdata_type_name,
          orka_err_from_factory // This will use the Display impl of orka_err_from_factory
        ),
      }
    })
  }
}
