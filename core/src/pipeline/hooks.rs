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
  AnyContextDataExtractor,
  ContextDataExtractorImpl,
  FinishHandler,
  Handler,
};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::core::trace::RunOutcome;
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
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
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
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
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
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    let final_handler: Handler<TData, Err> = Box::new(move |ctx_data| Box::pin(handler_fn(ctx_data)));
    self.after.entry(step_name.to_string()).or_default().push(final_handler);
    self
  }


  /// Registers a run-level finish handler: an async "finally" awaited on **every exit of a
  /// full [`run`](Self::run)**, whether the pipeline completed, was stopped by a handler,
  /// or failed (including the missing-handler configuration error). It receives the final
  /// shared context and the run's [`RunOutcome`].
  ///
  /// This is the home for cleanup that must not be lost on the error path: releasing a
  /// lock, restoring a traffic drain, deleting a temp dir, compensating a half-applied
  /// change.
  ///
  /// Multiple finish handlers run in registration order, and all of them run even if one
  /// fails. Error policy: on a run that returned `Ok` (Completed or Stopped), the first
  /// finish-handler error becomes the run's error, since a cleanup failure on a success
  /// path must surface. On an already-failed run, finish-handler errors are logged via
  /// `tracing` and the original error is returned, since cleanup must not mask the real
  /// failure.
  ///
  /// The partial runners ([`run_step`](Self::run_step), [`run_from`](Self::run_from),
  /// [`run_until`](Self::run_until)) and [`resolve_plan`](Self::resolve_plan) never fire
  /// finish handlers; use `run()` when you want finish semantics.
  ///
  /// Finish handlers run *before* the context's
  /// [`resources`](ContextData::resources) bag is released, so a finalizer can still use
  /// a temp dir or a lock guard that the run is holding. Use `on_finish` for cleanup that
  /// must be awaited, and the resource bag for values that clean themselves up in `Drop`.
  pub fn on_finish<F>(
    &mut self,
    handler_fn: impl Fn(ContextData<TData>, RunOutcome) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<(), Err>> + Send + 'static,
  {
    let final_handler: FinishHandler<TData, Err> =
      Box::new(move |ctx_data, outcome| Box::pin(handler_fn(ctx_data, outcome)));
    self.finish_handlers.push(final_handler);
    self
  }

  /// Removes every finish handler registered via [`on_finish`](Self::on_finish).
  ///
  /// [`stub_step`](Self::stub_step) deliberately leaves finish handlers alone; a test that
  /// wants to run without the cleanup ring drops it explicitly with this.
  pub fn clear_finish_handlers(&mut self) -> &mut Self {
    self.finish_handlers.clear();
    self
  }

  //
  // Granularity is replace-all per (step, phase): handlers are boxed closures with no
  // identity, so per-handler targeting is not expressible. The `clear_*`/`replace_*`
  // methods are surgical (they touch only the named phase); `stub_step` is the blessed
  // "make this whole step a no-op" path.

  /// Removes every `before` handler for the step. The step definition itself is untouched.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn clear_before(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.before.remove(step_name);
    self
  }

  /// Removes every `on` handler for the step, including any `on::<SData>` wrappers and any
  /// conditional master handler finalized onto it.
  ///
  /// Surgical: the step's extractor registration is left in place. If that orphans an
  /// extractor (nothing consumes it anymore), [`validate`](Self::validate) fails loudly,
  /// which is the correct error; re-register a consumer or call
  /// [`remove_extractor`](Self::remove_extractor). For whole-step neutralization use
  /// [`stub_step`](Self::stub_step) instead.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn clear_on(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.on.remove(step_name);
    self.sub_handler_steps.remove(step_name);
    self
  }

  /// Removes every `after` handler for the step. The step definition itself is untouched.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn clear_after(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.after.remove(step_name);
    self
  }

  /// Removes the step's extractor registration and sub-handler bookkeeping, if any.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn remove_extractor(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.extractors.remove(step_name);
    self.sub_handler_steps.remove(step_name);
    self
  }

  /// [`clear_before`](Self::clear_before) followed by [`before_root`](Self::before_root):
  /// the step ends up with exactly this `before` handler.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn replace_before_root<F>(
    &mut self,
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
    self.clear_before(step_name);
    self.before_root(step_name, handler_fn)
  }

  /// [`clear_on`](Self::clear_on) followed by [`on_root`](Self::on_root): the step ends up
  /// with exactly this `on` handler. This is the step-stubbing primitive for tests: the
  /// real handler set (including any conditional master handler) is dropped and the stub
  /// is all that remains in the `on` phase.
  ///
  /// Like `clear_on`, this leaves the step's extractor in place; see
  /// [`clear_on`](Self::clear_on) for the validate interaction.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn replace_on_root<F>(
    &mut self,
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
    self.clear_on(step_name);
    self.on_root(step_name, handler_fn)
  }

  /// [`clear_after`](Self::clear_after) followed by [`after_root`](Self::after_root): the
  /// step ends up with exactly this `after` handler.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn replace_after_root<F>(
    &mut self,
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
    self.clear_after(step_name);
    self.after_root(step_name, handler_fn)
  }

  /// Neutralizes a whole step: clears all three phases (dropping any conditional master
  /// handler along with everything else), removes the step's extractor and sub-handler
  /// bookkeeping, and installs a single `Continue` `on` handler so
  /// [`validate`](Self::validate) still passes and a trace still shows the step as
  /// completed.
  ///
  /// Finish handlers registered via [`on_finish`](Self::on_finish) are untouched; drop
  /// those explicitly with [`clear_finish_handlers`](Self::clear_finish_handlers).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn stub_step(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.clear_before(step_name);
    self.clear_on(step_name);
    self.clear_after(step_name);
    self.remove_extractor(step_name);
    self.on_root(step_name, |_ctx| async { Ok(PipelineControl::Continue) })
  }


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
    step_name: impl AsRef<str>,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync,
  {
    let step_name = step_name.as_ref();
    let extractor_impl = ContextDataExtractorImpl::<TData, SData>::new(extractor_fn);
    event!(Level::DEBUG, %step_name, sub_context_data_type = %std::any::type_name::<SData>(), "Extractor set.");
    self.set_extractor_impl(step_name, Arc::new(extractor_impl))
  }

  /// Registers a caller-supplied, type-erased extractor implementation. This is the
  /// injection seam behind [`set_extractor`](Self::set_extractor) and
  /// [`set_extractor_with_merge`](Self::set_extractor_with_merge): hand in your own
  /// [`AnyContextDataExtractor`] (a recording fake, an instrumented wrapper) and every
  /// `on::<SData>` handler registered for the step afterwards uses it.
  ///
  /// Replaces any previously registered extractor for the step. Note that `on::<SData>`
  /// captures the extractor at registration time, so replace the extractor **before**
  /// registering the handlers that should use it.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn set_extractor_impl(
    &mut self,
    step_name: impl AsRef<str>,
    extractor: Arc<dyn AnyContextDataExtractor<TData>>,
  ) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.extractors.insert(step_name.to_string(), extractor);
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
  /// The merge runs **only when the handler returns `Ok`**; a failed sub-handler leaves
  /// the root context untouched.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn set_extractor_with_merge<SData>(
    &mut self,
    step_name: impl AsRef<str>,
    extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static,
    merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync,
  {
    let step_name = step_name.as_ref();
    let extractor_impl = ContextDataExtractorImpl::<TData, SData>::with_merge(extractor_fn, merge_fn);
    event!(Level::DEBUG, %step_name, sub_context_data_type = %std::any::type_name::<SData>(), "Extractor with merge set.");
    self.set_extractor_impl(step_name, Arc::new(extractor_impl))
  }

  /// Registers an `on` hook that operates on the step's extracted `ContextData<SData>`.
  ///
  /// Annotate the closure parameter (`|sub: ContextData<MyType>|`): that is what tells
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
    step_name: impl AsRef<str>,
    handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static,
  ) -> &mut Self
  where
    SData: 'static + Send + Sync, // SData is the underlying data type for the sub-context
    F: Future<Output = Result<PipelineControl, Err>> + Send + 'static,
  {
    let step_name = step_name.as_ref();
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

    let wrapped_handler: Handler<TData, Err> = Box::new(move |root_ctx_data: ContextData<TData>| {
      let current_extractor = extractor_arc.clone();
      let user_sdata_handler = user_sdata_handler_arc.clone();
      let step_name_clone = step_name_for_handler.clone();

      Box::pin(async move {
        event!(Level::TRACE, step_name = %step_name_clone, "Executing wrapped on<SData> handler. Attempting extraction.");

        let any_sub_ctx_data = match current_extractor.extract_sub_context_data(root_ctx_data.clone()) {
          Ok(boxed_any) => boxed_any,
          Err(orka_extraction_err) => {
            event!(Level::ERROR, step_name = %step_name_clone, error = %orka_extraction_err, "Extractor function failed.");
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
            return Err(Err::from(final_err));
          }
        };

        let sub_sdata_ctx: ContextData<SData> = match downcast_context_data::<SData>(
          any_sub_ctx_data,
          current_extractor.sub_context_data_type_id(),
          &step_name_clone,
        ) {
          Ok(s_ctx_data) => s_ctx_data,
          Err(orka_downcast_err) => {
            event!(Level::ERROR, step_name = %step_name_clone, error = %orka_downcast_err, "Sub-context ContextData downcast failed.");
            return Err(Err::from(orka_downcast_err));
          }
        };
        event!(Level::TRACE, step_name = %step_name_clone, "Sub-context ContextData extraction and downcast successful.");

        // No lock guard is live across this await.
        event!(Level::TRACE, step_name = %step_name_clone, "Calling user's on<SData> handler.");
        let control = match (user_sdata_handler)(sub_sdata_ctx.clone()).await {
          Ok(control) => control,
          Err(handler_err) => {
            event!(Level::ERROR, step_name = %step_name_clone, error = %handler_err, "User's on<SData> handler failed.");
            // Deliberately skip the merge: a failed sub-handler leaves the root untouched.
            return Err(handler_err);
          }
        };

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
