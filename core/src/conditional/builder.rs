// orka/src/conditional/builder.rs

//! Implements the fluent builder API (`ConditionalScopeBuilder`, `ConditionalScopeConfigurator`)
//! for defining conditional execution of scoped pipelines within a main pipeline step.
//! The main pipeline is `Pipeline<TData, Err>` and its handlers return `Result<_, Err>`.
//! Scoped pipelines provided are now also `Pipeline<SData, Err>`.

use crate::conditional::provider::{FunctionalPipelineProvider, PipelineProvider, StaticPipelineProvider};
use crate::conditional::scope::{AnyConditionalScope, ConditionalScope}; // Now AnyConditionalScope<TData, Err>
use crate::core::context::Handler;
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::{OrkaError, OrkaResult}; // OrkaResult is Result<_, OrkaError>, used by extractor
use crate::pipeline::Pipeline; // Pipeline<TData, Err> or Pipeline<SData, Err>

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::{event, instrument, Level};

/// Builder for defining conditional scopes for a specific step in a `Pipeline<TData, Err>`.
///
/// `TData` is the underlying data type of the main pipeline's context.
/// `Err` is the error type returned by the main pipeline's handlers AND by the scoped pipelines.
/// `Err` must be constructible `From<OrkaError>` to handle framework-level errors (e.g., extractor failure).
pub struct ConditionalScopeBuilder<'pipeline, TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pipeline: &'pipeline mut Pipeline<TData, Err>,
  step_name: String,
  // AnyConditionalScope<TData, Err> stores scopes whose execute_scoped_pipeline method
  // will effectively run a Pipeline<SData, Err> and thus return Result<_, Err>.
  collected_scopes: Vec<Arc<dyn AnyConditionalScope<TData, Err>>>,
  on_no_match_behavior: PipelineControl,
  // _phantom_err: PhantomData<Err>, // No longer needed as Err is in collected_scopes
}

impl<'pipeline, TData, Err> ConditionalScopeBuilder<'pipeline, TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pub(crate) fn new(pipeline: &'pipeline mut Pipeline<TData, Err>, step_name: String) -> Self {
    if !pipeline.steps.iter().any(|s| s.name == step_name) {
      pipeline.steps.push(crate::core::step::StepDef {
        name: step_name.clone(),
        optional: false,
        skip_if: None,
      });
    }
    Self {
      pipeline,
      step_name,
      collected_scopes: Vec::new(),
      on_no_match_behavior: PipelineControl::Continue,
    }
  }

  /// Adds a conditional scope that uses a statically provided `Arc<Pipeline<SData, Err>>`.
  ///
  /// `SData` (underlying data for scoped pipeline) must be `Send + Sync + 'static`.
  /// `extractor_fn` returns `Result<ContextData<SData>, OrkaError>` (i.e., `Result<_, OrkaError>`).
  pub fn add_static_scope<SData>(
    self,
    static_pipeline: Arc<Pipeline<SData, Err>>, // Scoped pipeline uses main Err
    // Extractor's own failure is an OrkaError. This will be converted to Err by AnyConditionalScope impl.
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, StaticPipelineProvider<SData, Err>>
  where
    SData: 'static + Send + Sync,
  {
    ConditionalScopeConfigurator {
      builder: self,
      provider: Arc::new(StaticPipelineProvider::new(static_pipeline)),
      extractor: Arc::new(extractor_fn),
      _phantom_sdata: PhantomData,
    }
  }

  /// Adds a conditional scope that uses a factory to get an `Arc<Pipeline<SData, Err>>`.
  ///
  /// `SData` (underlying data for scoped pipeline) must be `Send + Sync + 'static`.
  /// `pipeline_factory` output future resolves to `Result<Arc<Pipeline<SData, Err>>, OrkaError>`.
  ///   (The factory itself can fail with OrkaError, but the pipeline it yields uses `Err`).
  /// `extractor_fn` returns `Result<ContextData<SData>, OrkaError>`.
  pub fn add_dynamic_scope<SData, F, Fut>(
    self,
    pipeline_factory: F,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, FunctionalPipelineProvider<TData, SData, Err, F, Fut>>
  where
    SData: 'static + Send + Sync,
    F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Arc<Pipeline<SData, Err>>, OrkaError>> + Send + 'static,
  {
    ConditionalScopeConfigurator {
      builder: self,
      provider: Arc::new(FunctionalPipelineProvider::new(pipeline_factory)),
      extractor: Arc::new(extractor_fn),
      _phantom_sdata: PhantomData,
    }
  }

  pub fn if_no_scope_matches(mut self, behavior: PipelineControl) -> Self {
    self.on_no_match_behavior = behavior;
    self
  }

  #[instrument(
        name = "ConditionalScopeBuilder::finalize_conditional_step",
        skip_all,
        fields(step_name = %self.step_name, num_scopes = self.collected_scopes.len())
    )]
  pub fn finalize_conditional_step(self, optional_for_main_step: bool) {
    let step_name_captured = self.step_name.clone();
    let collected_scopes_for_config = Arc::new(self.collected_scopes); // Vec<Arc<dyn AnyConditionalScope<TData, Err>>>
    let scopes_for_closure_capture = collected_scopes_for_config.clone();
    let on_no_match_behavior_captured = self.on_no_match_behavior;

    // The master handler is Handler<TData, Err> for the main pipeline.
    let master_handler: Handler<TData, Err> = Box::new(move |main_ctx_data: ContextData<TData>| {
      let scopes_to_check = scopes_for_closure_capture.clone();
      let step_name_log_ctx = step_name_captured.clone();
      let current_main_ctx_data = main_ctx_data.clone();
      // Capture the optional_for_main_step flag for use in the handler
      let is_step_optional_captured = optional_for_main_step;

      Box::pin(async move {
        for scope_candidate in scopes_to_check.iter() {
          if scope_candidate.is_condition_met(current_main_ctx_data.clone()) {
            event!(Level::DEBUG, step_name = %step_name_log_ctx, "Conditional scope matched. Executing.");

            match scope_candidate
              .execute_scoped_pipeline(current_main_ctx_data.clone())
              .await
            {
              Ok(control) => return Ok(control),
              Err(e) => {
                // An Err (MainErr) was returned from the conditional scope execution
                event!(Level::ERROR, step_name = %step_name_log_ctx, error = %e, "Error during conditional scope execution.");
                if is_step_optional_captured {
                  event!(Level::WARN, step_name = %step_name_log_ctx, "Conditional step is optional, swallowing error and continuing main pipeline.");
                  return Ok(PipelineControl::Continue); // Swallow error and continue
                } else {
                  return Err(e); // Propagate error if step is not optional
                }
              }
            }
          }
        }
        event!(Level::DEBUG, step_name = %step_name_log_ctx, "No conditional scope matched. Defaulting to {:?}.", on_no_match_behavior_captured);
        Ok(on_no_match_behavior_captured)
      })
    });

    // Update the main pipeline
    if let Some(step_def) = self.pipeline.steps.iter_mut().find(|s| s.name == self.step_name) {
      step_def.optional = optional_for_main_step;
    } else {
      event!(Level::WARN, step_name = %self.step_name, "Step definition not found during finalize_conditional_step. This may indicate an internal issue.");
    }

    // Store the collected scopes (Arc<Vec<Arc<dyn AnyConditionalScope<TData, Err>>>>)
    // and no-match behavior in the pipeline's config.
    let mut config_map_locked = self.pipeline.conditional_scopes_config.lock().unwrap();
    config_map_locked.insert(
      self.step_name.clone(),
      (collected_scopes_for_config, on_no_match_behavior_captured),
    );
    drop(config_map_locked);

    // Register the master handler for the 'on' phase of this step.
    self.pipeline.on.insert(self.step_name.clone(), vec![master_handler]);

    event!(Level::INFO, step_name = %self.step_name, "Conditional scopes finalized and master handler registered.");
  }
}

/// Intermediate builder to configure a single conditional scope.
/// `TData`: Main pipeline's underlying context data type.
/// `SData`: Scoped pipeline's underlying context data type. Must be `Send + Sync + 'static`.
/// `Err`: Error type of the main pipeline AND scoped pipelines. Must be `From<OrkaError>`.
/// `P`: Concrete `PipelineProvider<TData, SData, Err>` (provides `Pipeline<SData, Err>`).
pub struct ConditionalScopeConfigurator<
  'pipeline,
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  P: PipelineProvider<TData, SData, Err> + 'static, // P is PipelineProvider<TData, SData, Err>
> {
  builder: ConditionalScopeBuilder<'pipeline, TData, Err>,
  provider: Arc<P>,
  // extractor_fn returns Result<ContextData<SData>, OrkaError>
  extractor: Arc<dyn Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static>,
  _phantom_sdata: PhantomData<SData>, // To mark usage of SData
}

impl<'pipeline, TData, SData, Err, P> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, P>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  P: PipelineProvider<TData, SData, Err> + 'static,
{
  /// Sets the condition for this scope. `condition_fn` takes `ContextData<TData>`.
  /// Returns `ConditionalScopeBuilder<TData, Err>`.
  #[instrument(
        name = "ConditionalScopeConfigurator::on_condition",
        skip_all,
        fields(builder_step_name = %self.builder.step_name)
    )]
  pub fn on_condition(
    mut self,
    condition_fn: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static,
  ) -> ConditionalScopeBuilder<'pipeline, TData, Err> {
    // ConditionalScope<TData, SData, Err>
    let final_scope_definition = ConditionalScope::<TData, SData, Err> {
      pipeline_provider: self.provider, // This is Arc<dyn PipelineProvider<TData, SData, Err>>
      extractor: self.extractor,
      condition: Arc::new(condition_fn),
      _phantom_main_err: PhantomData, // From ConditionalScope struct
    };

    event!(Level::DEBUG, "Conditional scope configured with condition.");
    // builder.collected_scopes is Vec<Arc<dyn AnyConditionalScope<TData, Err>>>
    self.builder.collected_scopes.push(Arc::new(final_scope_definition));
    self.builder
  }
}
