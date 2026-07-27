//! Defines `ConditionalScope` which represents one potential execution path
//! within a conditional step, and `AnyConditionalScope` for type erasure.
//! Operates on `ContextData<TData>` and `ContextData<SData>`.

use crate::conditional::provider::PipelineProvider;
use crate::core::context::{ConditionFn, ExtractorFn, MergeFn};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::OrkaError;
use async_trait::async_trait;
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::{event, instrument, Level};

/// Represents a single conditional execution path (a "scope") within a step.
/// It pairs a condition with a way to get a scoped pipeline and an extractor for its context data.
///
/// `TData` is the main pipeline's underlying context data type.
/// `SData` is the scoped pipeline's underlying context data type.
/// `MainErr` is the error type for the scoped pipeline and for reporting errors back to the main pipeline.
/// Operations use `ContextData<TData>` and `ContextData<SData>`.
pub(crate) struct ConditionalScope<TData, SData, MainErr>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Provides the `Arc<Pipeline<SData, MainErr>>` instance to be executed if the condition is met.
  pub(crate) pipeline_provider: Arc<dyn PipelineProvider<TData, SData, MainErr>>,

  /// Extracts the sub-context `ContextData<SData>` from the main context `ContextData<TData>`.
  /// The extractor itself can fail with an `OrkaError`.
  pub(crate) extractor: ExtractorFn<TData, SData>,

  /// The condition (evaluated on `ContextData<TData>`) that determines if this scope should run.
  pub(crate) condition: ConditionFn<TData>,

  /// Optional write-back, folding the scoped context into the main context after the scoped
  /// pipeline finishes successfully. `None` leaves the scope detached (the historical behaviour).
  pub(crate) merge: Option<MergeFn<TData, SData>>,

  pub(crate) _phantom_main_err: PhantomData<MainErr>,
}

/// Type-erased trait for a conditional scope, allowing different `SData` types
/// to be stored heterogeneously.
///
/// `TData` is the main pipeline's underlying context data type.
/// `MainErr` is the error type of the main pipeline, which will also be the error type
/// returned by `execute_scoped_pipeline`.
#[async_trait]
pub(crate) trait AnyConditionalScope<TData, MainErr>: Send + Sync
where
  TData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Checks if this scope's condition is met given the main context data.
  fn is_condition_met(&self, main_ctx_data: ContextData<TData>) -> bool;

  /// If the condition is met, this method:
  /// 1. Gets the scoped pipeline instance (`Arc<Pipeline<SData, MainErr>>`) via its provider.
  ///    (Provider can fail with `OrkaError`, which is then converted to `MainErr`).
  /// 2. Extracts the sub-context data (`ContextData<SData>`).
  ///    (Extractor can fail with `OrkaError`, which is then converted to `MainErr`).
  /// 3. Executes the scoped pipeline with `ContextData<SData>`.
  ///    (Scoped pipeline execution returns `Result<_, MainErr>`).
  /// 4. Returns `Result<PipelineControl, MainErr>`.
  async fn execute_scoped_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<PipelineControl, MainErr>;
}

#[async_trait]
impl<TData, SData, MainErr> AnyConditionalScope<TData, MainErr> for ConditionalScope<TData, SData, MainErr>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  fn is_condition_met(&self, main_ctx_data: ContextData<TData>) -> bool {
    (self.condition)(main_ctx_data)
  }

  #[instrument(
        name = "AnyConditionalScope::execute_scoped_pipeline",
        skip(self, main_ctx_data),
        fields(
            main_context_data_type = %std::any::type_name::<TData>(),
            scoped_context_data_type = %std::any::type_name::<SData>(),
            main_error_type = %std::any::type_name::<MainErr>(),
        ),
        err(Display)
    )]
  async fn execute_scoped_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<PipelineControl, MainErr> {
    event!(Level::DEBUG, "Attempting to execute conditional scope.");

    let scoped_pipeline_instance = match self.pipeline_provider.get_pipeline(main_ctx_data.clone()).await {
      Ok(p) => {
        event!(Level::TRACE, "Scoped pipeline instance obtained.");
        p
      }
      Err(orka_provider_err) => {
        event!(Level::ERROR, error = %orka_provider_err, "Failed to get pipeline from provider.");
        let enriched_err = match orka_provider_err {
          OrkaError::HandlerError { source } => OrkaError::PipelineProviderFailure {
            step_name: String::from("conditional_scope_provider"), // Step name not directly known here
            source,
          },
          OrkaError::PipelineProviderFailure { source, .. } => OrkaError::PipelineProviderFailure {
            step_name: String::from("conditional_scope_provider"),
            source,
          },
          other_err => other_err,
        };
        return Err(MainErr::from(enriched_err));
      }
    };

    let sub_sdata_ctx: ContextData<SData> = match (self.extractor)(main_ctx_data.clone()) {
      Ok(s_ctx_data) => {
        event!(Level::TRACE, "Sub-context data extracted successfully.");
        s_ctx_data
      }
      Err(orka_extractor_err) => {
        event!(Level::ERROR, error = %orka_extractor_err, "Sub-context data extractor failed.");
        let enriched_err = match orka_extractor_err {
          OrkaError::HandlerError { source } => OrkaError::ExtractorFailure {
            step_name: String::from("conditional_scope_extractor"), // Step name not known here
            source,
          },
          OrkaError::ExtractorFailure { source, .. } => OrkaError::ExtractorFailure {
            step_name: String::from("conditional_scope_extractor"),
            source,
          },
          other_err => other_err,
        };
        return Err(MainErr::from(enriched_err));
      }
    };

    event!(Level::DEBUG, "Running scoped pipeline.");
    let control = match scoped_pipeline_instance.run(sub_sdata_ctx.clone()).await {
      Ok(crate::core::control::PipelineResult::Completed) => {
        event!(Level::INFO, "Scoped pipeline completed successfully.");
        PipelineControl::Continue
      }
      Ok(crate::core::control::PipelineResult::Stopped) => {
        event!(Level::INFO, "Scoped pipeline was stopped by one of its handlers.");
        PipelineControl::Stop
      }
      Err(main_err_from_scoped_pipeline) => {
        event!(Level::ERROR, error = %main_err_from_scoped_pipeline, "Scoped pipeline execution failed.");
        // Deliberately skip the merge: a failed scope leaves the main context untouched.
        return Err(main_err_from_scoped_pipeline);
      }
    };

    //    Both guards are taken and dropped inside this block; no `.await` happens here.
    if let Some(merge) = self.merge.as_ref() {
      event!(Level::TRACE, "Merging scoped context back into main context.");
      let sub_guard = sub_sdata_ctx.read();
      let mut main_guard = main_ctx_data.write();
      merge(&mut *main_guard, &*sub_guard);
    }

    Ok(control)
  }
}
