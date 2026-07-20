//! Contains methods for registering `before`, `on`, and `after` handlers
//! for pipeline steps.
//!
//! Handlers take a `ContextData<TData>` (or `ContextData<SData>` for sub-context
//! handlers) and return a future resolving to `Result<PipelineControl, Err>`, where
//! `Err` is the pipeline's own error type. Because `Err` is fixed, a plain `Ok(...)`
//! infers correctly and `?` converts other error types through `From` as usual:
//!
//! ```ignore
//! pipeline
//!   .on_root("load", |ctx| async move {
//!     let cfg = std::fs::read_to_string("cfg.toml")?; // converts via From<io::Error>
//!     ctx.write().config = cfg;
//!     Ok(PipelineControl::Continue)
//!   })
//!   .on_root("notify", |ctx| async move {
//!     Ok(PipelineControl::Continue)
//!   });
//! ```

use tracing::{event, instrument, Level};

use crate::core::context::{
  downcast_context_data,
  ContextDataExtractorImpl,
  Handler, // Handler<TData, Err>
};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline;
use std::future::Future;
use std::sync::Arc;

impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Registers a `before` hook for a step. Returns `&mut Self` so registrations chain.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn before_root<F>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    self.ensure_step_exists(step_name);
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| Box::pin(handler_fn(ctx_data)));
    self
      .before
      .entry(step_name.to_string())
      .or_default()
      .push(final_handler);
    self
  }

  /// Registers an `on` hook for a step. Returns `&mut Self` so registrations chain.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn on_root<F>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    self.ensure_step_exists(step_name);
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| Box::pin(handler_fn(ctx_data)));
    self.on.entry(step_name.to_string()).or_default().push(final_handler);
    self
  }

  /// Registers an `after` hook for a step. Returns `&mut Self` so registrations chain.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn after_root<F>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    self.ensure_step_exists(step_name);
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| Box::pin(handler_fn(ctx_data)));
    self.after.entry(step_name.to_string()).or_default().push(final_handler);
    self
  }

  // --- Sub-Context Handlers (using ContextData) ---

  /// Registers an extractor producing a `ContextData<SData>` sub-context for a step.
  ///
  /// The sub-context is **detached**: it is a separate `ContextData`, so writes made by
  /// the `on::<SData>` handler are *not* reflected in the root context. Use
  /// [`set_extractor_with_merge`](Self::set_extractor_with_merge) when you need them back.
  ///
  /// [`ContextData::project`] is the usual way to write one of these.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn set_extractor<SData>(
    &mut self,
    step_name: &str,
    // Extractor's own failure is an OrkaError. This is consistent.
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync,
  {
    self.ensure_step_exists(step_name);
    let extractor_impl = ContextDataExtractorImpl::<TData, SData>::new(extractor_fn);
    self.extractors.insert(step_name.to_string(), Arc::new(extractor_impl));
    event!(Level::DEBUG, %step_name, sub_context_data_type = %std::any::type_name::<SData>(), "Extractor set.");
    self
  }

  /// Registers an extractor that also folds the sub-context back into the root context.
  ///
  /// After the step's `on::<SData>` handler succeeds, `merge_fn` runs with a write lock on
  /// the root context and a read lock on the sub-context, letting the sub-pipeline's work
  /// land in the parent:
  ///
  /// ```ignore
  /// pipeline
  ///   .set_extractor_with_merge(
  ///     "validate",
  ///     |main| Ok(main.project(|d| d.customer.clone())),
  ///     |root, sub| root.customer = sub.clone(),
  ///   )
  ///   .on("validate", |sub: ContextData<Customer>| async move {
  ///     sub.write().is_validated = true;
  ///     Ok(PipelineControl::Continue)
  ///   });
  /// ```
  ///
  /// The merge runs **only when the handler returns `Ok`** — a failed sub-handler leaves
  /// the root context untouched.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn set_extractor_with_merge<SData>(
    &mut self,
    step_name: &str,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
    merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync,
  {
    self.ensure_step_exists(step_name);
    let extractor_impl = ContextDataExtractorImpl::<TData, SData>::with_merge(extractor_fn, merge_fn);
    self.extractors.insert(step_name.to_string(), Arc::new(extractor_impl));
    event!(Level::DEBUG, %step_name, sub_context_data_type = %std::any::type_name::<SData>(), "Extractor with merge set.");
    self
  }

  /// Registers an `on` hook that operates on the step's extracted `ContextData<SData>`.
  ///
  /// Annotate the closure parameter (`|sub: ContextData<MyType>|`) — that is what tells
  /// Orka which `SData` you mean.
  ///
  /// # Panics
  /// Panics if the step does not exist, or if no extractor has been registered for it via
  /// [`set_extractor`](Self::set_extractor) / [`set_extractor_with_merge`](Self::set_extractor_with_merge).
  #[instrument(
        name = "Pipeline::on<SData>",
        skip_all,
        fields(step_name, sub_context_data_type = %std::any::type_name::<SData>())
    )]
  pub fn on<SData, F>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync, // SData is the underlying data type for the sub-context
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    self.ensure_step_exists(step_name);

    let extractor_arc = self.extractors.get(step_name).cloned().unwrap_or_else(|| {
      panic!(
        "Orka setup error: No extractor found for step '{}' when registering on<{}> handler. Call set_extractor first.",
        step_name,
        std::any::type_name::<SData>()
      )
    });

    let step_name_for_handler = step_name.to_string();
    let user_sdata_handler_arc = Arc::new(handler_fn);

    // The wrapped handler is Handler<TData, Err>
    let wrapped_handler: Handler<TData, Err> = Box::new(move |root_ctx_data: ContextData<TData>| {
      let current_extractor = extractor_arc.clone();
      let user_sdata_handler = user_sdata_handler_arc.clone();
      let step_name_clone = step_name_for_handler.clone();

      Box::pin(async move {
        event!(Level::TRACE, step_name = %step_name_clone, "Executing wrapped on<SData> handler. Attempting extraction.");

        // 1. Extraction yields OrkaResult<Box<dyn Any + Send>> (i.e. Result<_, OrkaError>)
        //    We need to map OrkaError to Err if extraction fails.
        let any_sub_ctx_data = match current_extractor.extract_sub_context_data(root_ctx_data.clone()) {
          Ok(boxed_any) => boxed_any,
          Err(orka_extraction_err) => {
            // This is an OrkaError
            event!(Level::ERROR, step_name = %step_name_clone, error = %orka_extraction_err, "Extractor function failed.");
            // Enrich OrkaError if needed, then convert to Err
            let final_err = match orka_extraction_err {
              OrkaError::HandlerError { source } => OrkaError::ExtractorFailure {
                step_name: step_name_clone.clone(),
                source,
              },
              OrkaError::ExtractorFailure { source, step_name: _ } => OrkaError::ExtractorFailure {
                step_name: step_name_clone.clone(),
                source,
              },
              other_err => other_err,
            };
            return Err(Err::from(final_err)); // Convert OrkaError to main pipeline's Err
          }
        };

        // 2. Downcast also yields OrkaResult (Result<_, OrkaError>), map OrkaError to Err
        let sub_sdata_ctx: ContextData<SData> = match downcast_context_data::<SData>(
          any_sub_ctx_data,
          current_extractor.sub_context_data_type_id(),
          &step_name_clone,
        ) {
          Ok(s_ctx_data) => s_ctx_data,
          Err(orka_downcast_err) => {
            // This is an OrkaError
            event!(Level::ERROR, step_name = %step_name_clone, error = %orka_downcast_err, "Sub-context ContextData downcast failed.");
            return Err(Err::from(orka_downcast_err)); // Convert OrkaError to main pipeline's Err
          }
        };
        event!(Level::TRACE, step_name = %step_name_clone, "Sub-context ContextData extraction and downcast successful.");

        // 3. Call user's SData handler. No lock guard is live across this await.
        event!(Level::TRACE, step_name = %step_name_clone, "Calling user's on<SData> handler.");
        let control = match (user_sdata_handler)(sub_sdata_ctx.clone()).await {
          Ok(control) => control,
          Err(handler_err) => {
            event!(Level::ERROR, step_name = %step_name_clone, error = %handler_err, "User's on<SData> handler failed.");
            // Deliberately skip the merge: a failed sub-handler leaves the root untouched.
            return Err(handler_err);
          }
        };

        // 4. Fold the sub-context back into the root, if this extractor was given a merge fn.
        //    No `.await` occurs while the guards taken inside are held.
        if current_extractor.has_merge() {
          event!(Level::TRACE, step_name = %step_name_clone, "Merging sub-context back into root context.");
          if let Err(merge_err) = current_extractor.merge_sub_context_data(root_ctx_data, &sub_sdata_ctx) {
            event!(Level::ERROR, step_name = %step_name_clone, error = %merge_err, "Merging sub-context back failed.");
            return Err(Err::from(merge_err));
          }
        }

        Ok(control)
      })
    });

    self.on.entry(step_name.to_string()).or_default().push(wrapped_handler);
    self.sub_handler_steps.insert(step_name.to_string());
    event!(Level::DEBUG, "on<SData> handler registered.");
    self
  }
}
