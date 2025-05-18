// orka/src/pipeline/hooks.rs

//! Contains methods for registering `before`, `on`, and `after` handlers
//! for pipeline steps. Handlers for `Pipeline<TData, Err>` operate on
//! `ContextData<TData>` and return `Result<_, Err>`, or on `ContextData<SData>`
//! for sub-context handlers, also ultimately resulting in `Result<_, Err>`.

use tracing::{event, instrument, Level};

use crate::core::context::{
  downcast_context_data,
  AnyContextDataExtractor,
  ContextDataExtractorImpl,
  Handler, // Handler<TData, Err>
};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::{OrkaError, OrkaResult}; // OrkaResult is Result<_, OrkaError> - used by extractor
use crate::pipeline::definition::Pipeline; // This is Pipeline<TData, Err> where Err already has From<OrkaError> from its struct def
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// This impl block needs to ensure Err satisfies all bounds required by the Pipeline struct
// and any methods it calls from other impl blocks of Pipeline.
// Since Pipeline<TData, Err> struct definition requires Err: From<OrkaError>,
// this impl block must also reflect that for consistency and to call methods
// defined in other impl blocks that rely on this bound.
impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  // This bound ensures that methods like `ensure_step_exists` (which are defined
  // under an impl block requiring Err: From<OrkaError> because Pipeline struct requires it)
  // are callable.
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Registers a `before` hook for a given step.
  ///
  /// The `handler_fn` takes `ContextData<TData>` and returns a `Future`
  /// resolving to `Result<PipelineControl, UserProvidedErr>`, where
  /// `UserProvidedErr` must be convertible into the pipeline's `Err` type.
  pub fn before_root<F, UserProvidedErr>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) where
    F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static,
    UserProvidedErr: Into<Err> + Send + Sync + 'static, // User's error converts to pipeline's Err
  {
    self.ensure_step_exists(step_name); // Now callable due to Err: From<OrkaError> on this impl block
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| {
      let user_fut = handler_fn(ctx_data);
      Box::pin(async move { user_fut.await.map_err(Into::into) }) // UserProvidedErr.into() -> Err
    });
    self
      .before
      .entry(step_name.to_string())
      .or_default()
      .push(final_handler);
  }

  /// Registers an `on` hook for a given step.
  /// (Similar to `before_root` regarding error types).
  pub fn on_root<F, UserProvidedErr>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) where
    F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static,
    UserProvidedErr: Into<Err> + Send + Sync + 'static,
  {
    self.ensure_step_exists(step_name); // Now callable
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| {
      let user_fut = handler_fn(ctx_data);
      Box::pin(async move { user_fut.await.map_err(Into::into) })
    });
    self.on.entry(step_name.to_string()).or_default().push(final_handler);
  }

  /// Registers an `after` hook for a given step.
  /// (Similar to `before_root` regarding error types).
  pub fn after_root<F, UserProvidedErr>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) where
    F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static,
    UserProvidedErr: Into<Err> + Send + Sync + 'static,
  {
    self.ensure_step_exists(step_name); // Now callable
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| {
      let user_fut = handler_fn(ctx_data);
      Box::pin(async move { user_fut.await.map_err(Into::into) })
    });
    self.after.entry(step_name.to_string()).or_default().push(final_handler);
  }

  // --- Sub-Context Handlers (using ContextData) ---

  /// Registers an extractor from `ContextData<TData>` to `ContextData<SData>`.
  ///
  /// The `extractor_fn` returns `Result<ContextData<SData>, OrkaError>` (i.e., `Result<_, OrkaError>`).
  /// This is because extractor failure is a framework-level concern.
  /// `SData` must be `Send + Sync + 'static`.
  pub fn set_extractor<SData>(
    &mut self,
    step_name: &str,
    // Extractor's own failure is an OrkaError. This is consistent.
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) where
    SData: 'static + Send + Sync,
  {
    self.ensure_step_exists(step_name); // Now callable
    let extractor_impl = ContextDataExtractorImpl::<TData, SData>::new(extractor_fn);
    self.extractors.insert(step_name.to_string(), Arc::new(extractor_impl));
    event!(Level::DEBUG, %step_name, sub_context_data_type = %std::any::type_name::<SData>(), "Extractor set.");
  }

  /// Registers an `on` hook for a given step, operating on an extracted `ContextData<SData>`.
  ///
  /// An extractor (via `set_extractor`) must have been registered for this step.
  /// The sub-handler `handler_fn` takes `ContextData<SData>` and returns
  /// `Result<PipelineControl, SubHandlerErr>`, where `SubHandlerErr` must be convertible
  /// into the main pipeline's `Err` type.
  ///
  /// The `Err` type of the main pipeline (already constrained by this `impl` block to be `From<OrkaError>`)
  /// is used to handle potential errors from the extraction process itself (which are `OrkaError`).
  #[instrument(
        name = "Pipeline::on<SData>",
        skip_all,
        fields(step_name, sub_context_data_type = %std::any::type_name::<SData>())
    )]
  pub fn on<SData, F, SubHandlerErr>(
    &mut self,
    step_name: &str,
    handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static,
  ) where
    SData: 'static + Send + Sync, // SData is the underlying data type for the sub-context
    F: Future<Output = Result<PipelineControl, SubHandlerErr>> + Send + 'static,
    SubHandlerErr: Into<Err> + Send + Sync + 'static + std::fmt::Debug, // Sub-handler's error converts to main pipeline's Err
                                                                        // The `Err: From<OrkaError>` bound is already present on the `impl` block.
                                                                        // No need to restate it here.
  {
    self.ensure_step_exists(step_name); // Now callable

    let extractor_arc = self.extractors.get(step_name).cloned().unwrap_or_else(|| {
      panic!(
        "Orka panic: No extractor found for step '{}' when registering on<{}> handler. Call set_extractor first.",
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
        let any_sub_ctx_data = match current_extractor.extract_sub_context_data(root_ctx_data) {
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

        // 3. Call user's SData handler, which returns Result<_, SubHandlerErr>. Map SubHandlerErr to Err.
        event!(Level::TRACE, step_name = %step_name_clone, "Calling user's on<SData> handler.");
        (user_sdata_handler)(sub_sdata_ctx).await.map_err(|sub_err| {
          // sub_err is SubHandlerErr
          event!(Level::ERROR, step_name = %step_name_clone, error = ?sub_err, "User's on<SData> handler failed.");
          // Convert SubHandlerErr to main pipeline's Err
          sub_err.into()
        })
      })
    });

    self.on.entry(step_name.to_string()).or_default().push(wrapped_handler);
    event!(Level::DEBUG, "on<SData> handler registered.");
  }
}
