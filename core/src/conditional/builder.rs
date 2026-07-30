//! Implements the fluent builder API (`ConditionalScopeBuilder`, `ConditionalScopeConfigurator`)
//! for defining conditional execution of scoped pipelines within a main pipeline step.
//! The main pipeline is `Pipeline<TData, Err>` and its handlers return `Result<_, Err>`.
//! Scoped pipelines provided are now also `Pipeline<SData, Err>`.

use crate::conditional::provider::{DynPipelineProvider, FunctionalPipelineProvider, PipelineProvider, StaticPipelineProvider};
use crate::conditional::scope::{AnyConditionalScope, ConditionalScope};
use crate::core::context::{ExtractorFn, Handler, MergeFn};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::OrkaError;
use crate::pipeline::Pipeline;

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::{event, instrument, Level};

/// Builder for defining conditional scopes for a specific step in a `Pipeline<TData, Err>`.
///
/// `TData` is the underlying data type of the main pipeline's context.
/// `Err` is the error type returned by the main pipeline's handlers AND by the scoped pipelines.
/// `Err` must be constructible `From<OrkaError>` to handle framework-level errors (e.g., extractor failure).
#[must_use = "conditional scopes are only applied when you call .finalize_conditional_step()"]
pub struct ConditionalScopeBuilder<'pipeline, TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  pipeline: &'pipeline mut Pipeline<TData, Err>,
  step_name: String,
  collected_scopes: Vec<Arc<dyn AnyConditionalScope<TData, Err>>>,
  on_no_match_behavior: PipelineControl,
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
        skip_label: None,
      });
    }
    // Cleared again by `finalize_conditional_step`; anything left here is reported by
    // `Pipeline::validate` as a scope configuration that was silently discarded.
    pipeline.pending_conditional.insert(step_name.clone());
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
    static_pipeline: Arc<Pipeline<SData, Err>>,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, StaticPipelineProvider<SData, Err>>
  where
    SData: 'static + Send + Sync,
  {
    ConditionalScopeConfigurator {
      builder: self,
      provider: Arc::new(StaticPipelineProvider::new(static_pipeline)),
      extractor: Arc::new(extractor_fn),
      merge: None,
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
      merge: None,
      _phantom_sdata: PhantomData,
    }
  }

  /// Adds a conditional scope backed by a caller-supplied [`PipelineProvider`]
  /// implementation, as a trait object.
  ///
  /// This is the generalization of [`add_static_scope`](Self::add_static_scope) and
  /// [`add_dynamic_scope`](Self::add_dynamic_scope) (both are just concrete provider
  /// types) and the injection point for fakes in tests: a recording provider that counts
  /// invocations, or one that yields a canned scoped pipeline.
  pub fn add_scope_with_provider<SData>(
    self,
    provider: Arc<dyn PipelineProvider<TData, SData, Err>>,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, DynPipelineProvider<TData, SData, Err>>
  where
    SData: 'static + Send + Sync,
  {
    ConditionalScopeConfigurator {
      builder: self,
      provider: Arc::new(DynPipelineProvider::new(provider)),
      extractor: Arc::new(extractor_fn),
      merge: None,
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
    let scopes_for_closure_capture = Arc::new(self.collected_scopes);
    let on_no_match_behavior_captured = self.on_no_match_behavior;
    // The shared observer slot, not a snapshot: a tracer attached to the pipeline after
    // this finalize call must still be seen by the master handler.
    let observer_slot = self.pipeline.observer.clone();

    let master_handler: Handler<TData, Err> = Box::new(move |main_ctx_data: ContextData<TData>| {
      let scopes_to_check = scopes_for_closure_capture.clone();
      let step_name_log_ctx = step_name_captured.clone();
      let current_main_ctx_data = main_ctx_data.clone();
      let observer_slot = observer_slot.clone();
      let is_step_optional_captured = optional_for_main_step;

      Box::pin(async move {
        // One short lock before any await; the execution loop's WithRunId wrapper makes
        // the current run id ambient while this handler is polled.
        let observer = observer_slot.lock().clone();
        let run_id = crate::core::trace::current_run_id();
        let emit = |kind: crate::core::trace::TraceEventKind| {
          if let Some(obs) = &observer {
            obs.on_event(&crate::core::trace::TraceEvent { run_id, kind });
          }
        };

        for (scope_index, scope_candidate) in scopes_to_check.iter().enumerate() {
          if scope_candidate.is_condition_met(current_main_ctx_data.clone()) {
            event!(Level::DEBUG, step_name = %step_name_log_ctx, "Conditional scope matched. Executing.");
            emit(crate::core::trace::TraceEventKind::ScopeMatched {
              step: step_name_log_ctx.clone(),
              scope_index,
            });

            match scope_candidate
              .execute_scoped_pipeline(current_main_ctx_data.clone())
              .await
            {
              Ok(control) => return Ok(control),
              Err(e) => {
                event!(Level::ERROR, step_name = %step_name_log_ctx, error = %e, "Error during conditional scope execution.");
                if is_step_optional_captured {
                  event!(Level::WARN, step_name = %step_name_log_ctx, "Conditional step is optional, swallowing error and continuing main pipeline.");
                  return Ok(PipelineControl::Continue);
                } else {
                  return Err(e);
                }
              }
            }
          }
        }
        event!(Level::DEBUG, step_name = %step_name_log_ctx, "No conditional scope matched. Defaulting to {:?}.", on_no_match_behavior_captured);
        emit(crate::core::trace::TraceEventKind::ScopeNotMatched {
          step: step_name_log_ctx.clone(),
        });
        Ok(on_no_match_behavior_captured)
      })
    });

    if let Some(step_def) = self.pipeline.steps.iter_mut().find(|s| s.name == self.step_name) {
      step_def.optional = optional_for_main_step;
    } else {
      event!(Level::WARN, step_name = %self.step_name, "Step definition not found during finalize_conditional_step. This may indicate an internal issue.");
    }

    // This step is now properly configured; drop it from the "never finalized" set.
    self.pipeline.pending_conditional.remove(&self.step_name);

    // Append (do NOT replace) the master handler for the 'on' phase of this step, so
    // conditional scopes compose with any `on_root` handlers already registered here.
    self
      .pipeline
      .on
      .entry(self.step_name.clone())
      .or_default()
      .push(master_handler);

    event!(Level::INFO, step_name = %self.step_name, "Conditional scopes finalized and master handler registered.");
  }
}

/// Intermediate builder to configure a single conditional scope.
/// `TData`: Main pipeline's underlying context data type.
/// `SData`: Scoped pipeline's underlying context data type. Must be `Send + Sync + 'static`.
/// `Err`: Error type of the main pipeline AND scoped pipelines. Must be `From<OrkaError>`.
/// `P`: Concrete `PipelineProvider<TData, SData, Err>` (provides `Pipeline<SData, Err>`).
#[must_use = "this scope is only registered when you call .on_condition(), and applied when you call .finalize_conditional_step()"]
pub struct ConditionalScopeConfigurator<
  'pipeline,
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  P: PipelineProvider<TData, SData, Err> + 'static,
> {
  builder: ConditionalScopeBuilder<'pipeline, TData, Err>,
  provider: Arc<P>,
  extractor: ExtractorFn<TData, SData>,
  merge: Option<MergeFn<TData, SData>>,
  _phantom_sdata: PhantomData<SData>,
}

impl<'pipeline, TData, SData, Err, P> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, P>
where
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  P: PipelineProvider<TData, SData, Err> + 'static,
{
  /// Folds this scope's context back into the main context after the scoped pipeline
  /// completes successfully.
  ///
  /// Without this, a scope is detached: the scoped pipeline works on its own `ContextData<SData>`
  /// and the main context sees nothing of what it did. With it, results land in the parent:
  ///
  /// ```ignore
  /// pipeline
  ///   .conditional_scopes_for_step("pay")
  ///   .add_static_scope(provider_a, |main| Ok(main.project(|d| d.payment.clone())))
  ///   .with_merge(|main, sub| main.payment = sub.clone())
  ///   .on_condition(|main| main.read().provider == Provider::A)
  ///   .finalize_conditional_step(false);
  /// ```
  ///
  /// The merge runs **only when the scoped pipeline succeeds**; a failed scope leaves the
  /// main context untouched.
  pub fn with_merge(mut self, merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static) -> Self {
    self.merge = Some(Arc::new(merge_fn));
    self
  }

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
    let final_scope_definition = ConditionalScope::<TData, SData, Err> {
      pipeline_provider: self.provider,
      extractor: self.extractor,
      condition: Arc::new(condition_fn),
      merge: self.merge,
      _phantom_main_err: PhantomData,
    };

    event!(Level::DEBUG, "Conditional scope configured with condition.");
    self.builder.collected_scopes.push(Arc::new(final_scope_definition));
    self.builder
  }
}
