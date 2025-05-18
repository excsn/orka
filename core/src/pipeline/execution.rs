// orka/src/pipeline/execution.rs

//! Contains the `Pipeline::run()` method, responsible for executing the pipeline's steps and handlers.
//! The pipeline is `Pipeline<TData, Err>`, and `run` returns `Result<PipelineResult, Err>`.

use crate::core::context_data::ContextData;
use crate::core::control::{PipelineControl, PipelineResult};
use crate::error::OrkaError; // Orka's specific error enum
use crate::pipeline::definition::Pipeline; // This is Pipeline<TData, Err> which needs Err: From<OrkaError>
                                           // use std::marker::PhantomData; // Not needed here
use tracing::{event, instrument, span, Level};

// This impl block must satisfy the bounds on the Pipeline struct.
impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  // Add the From<OrkaError> bound here, consistent with the struct definition
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Executes the pipeline against the given shared context `ctx_data`.
  ///
  /// Returns `Result<PipelineResult, Err>`, where `Err` is the error type
  /// configured for this pipeline's handlers.
  ///
  /// If the pipeline configuration itself leads to an error (e.g., a non-optional
  /// step has no handlers), an `OrkaError` is generated. This method uses the
  /// `Err: From<OrkaError>` bound (now on the impl block) to convert such
  /// framework errors into the pipeline's `Err` type.
  #[instrument(
        name = "Pipeline::run",
        skip_all,
        fields(
            pipeline_context_data_type = %std::any::type_name::<TData>(),
            pipeline_error_type = %std::any::type_name::<Err>(),
            num_steps = self.steps.len(),
        ),
        err(Display)
    )]
  pub async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>
// where Err: From<OrkaError>, // This bound is now on the impl block, no longer needed here.
  {
    event!(Level::DEBUG, "Pipeline execution starting.");

    for (step_idx, step_def) in self.steps.iter().enumerate() {
      let step_name_str = step_def.name.as_str();

      let step_span = span!(
        Level::INFO,
        "pipeline_step_execution",
        step_name = step_name_str,
        step_index = step_idx,
        optional = step_def.optional
      );
      let _step_span_guard = step_span.enter();
      event!(Level::DEBUG, "Processing step.");

      if let Some(skip_cond_fn) = &step_def.skip_if {
        // Ensure context data is cloned for the closure, as it might be used again.
        if skip_cond_fn(ctx_data.clone()) {
          event!(Level::INFO, "Step skipped due to 'skip_if' condition.");
          continue;
        }
      }

      let has_before_handlers = self.before.get(step_name_str).map_or(false, |v| !v.is_empty());
      let has_on_handlers = self.on.get(step_name_str).map_or(false, |v| !v.is_empty());
      let has_after_handlers = self.after.get(step_name_str).map_or(false, |v| !v.is_empty());

      if !has_before_handlers && !has_on_handlers && !has_after_handlers {
        if step_def.optional {
          event!(Level::DEBUG, "Optional step has no handlers, skipping.");
          continue;
        } else {
          event!(Level::ERROR, "Non-optional step has no handlers.");
          // Convert OrkaError::HandlerMissing to this pipeline's Err type.
          // Err::from works because Err: From<OrkaError> is on the impl block.
          return Err(Err::from(OrkaError::HandlerMissing {
            step_name: step_def.name.clone(),
          }));
        }
      }

      // BEFORE phase
      if let Some(handlers) = self.before.get(step_name_str) {
        if !handlers.is_empty() {
          event!(Level::TRACE, "Executing 'before' handlers.");
          for (handler_idx, handler_fn) in handlers.iter().enumerate() {
            let handler_span = span!(Level::DEBUG, "before_handler", handler_index = handler_idx);
            let _handler_span_guard = handler_span.enter();
            // handler_fn is from Handler<TData, Err>, so it returns Result<PipelineControl, Err>
            match handler_fn(ctx_data.clone()).await {
              // Clone ctx_data for each handler
              Ok(PipelineControl::Continue) => {}
              Ok(PipelineControl::Stop) => {
                event!(Level::INFO, "Pipeline stopped by a 'before' handler.");
                return Ok(PipelineResult::Stopped);
              }
              Err(e) => {
                // e is of type Err
                event!(Level::ERROR, error = %e, "'before' handler failed.");
                return Err(e);
              }
            }
          }
        }
      }

      // ON phase
      if let Some(handlers) = self.on.get(step_name_str) {
        if !handlers.is_empty() {
          event!(Level::TRACE, "Executing 'on' handlers.");
          for (handler_idx, handler_fn) in handlers.iter().enumerate() {
            let handler_span = span!(Level::DEBUG, "on_handler", handler_index = handler_idx);
            let _handler_span_guard = handler_span.enter();
            match handler_fn(ctx_data.clone()).await {
              // Clone ctx_data
              Ok(PipelineControl::Continue) => {}
              Ok(PipelineControl::Stop) => {
                event!(Level::INFO, "Pipeline stopped by an 'on' handler.");
                return Ok(PipelineResult::Stopped);
              }
              Err(e) => {
                // e is of type Err
                event!(Level::ERROR, error = %e, "'on' handler failed.");
                return Err(e);
              }
            }
          }
        }
      } else if !step_def.optional && !has_before_handlers && !has_after_handlers {
        // This implies a non-optional step was expected to have 'on' handlers but didn't.
        // This check might be slightly redundant if the earlier check for any handlers covers it,
        // but it's a specific case for 'on' handlers being primary.
        event!(
          Level::ERROR,
          "Non-optional step is missing 'on' handlers (and has no before/after)."
        );
        return Err(Err::from(OrkaError::HandlerMissing {
          // Err::from works
          step_name: step_def.name.clone(),
        }));
      }

      // AFTER phase
      if let Some(handlers) = self.after.get(step_name_str) {
        if !handlers.is_empty() {
          event!(Level::TRACE, "Executing 'after' handlers.");
          for (handler_idx, handler_fn) in handlers.iter().enumerate() {
            let handler_span = span!(Level::DEBUG, "after_handler", handler_index = handler_idx);
            let _handler_span_guard = handler_span.enter();
            match handler_fn(ctx_data.clone()).await {
              // Clone ctx_data
              Ok(PipelineControl::Continue) => {}
              Ok(PipelineControl::Stop) => {
                event!(Level::INFO, "Pipeline stopped by an 'after' handler.");
                return Ok(PipelineResult::Stopped);
              }
              Err(e) => {
                // e is of type Err
                event!(Level::ERROR, error = %e, "'after' handler failed.");
                return Err(e);
              }
            }
          }
        }
      }
      event!(Level::DEBUG, "Step processing finished successfully.");
    } // End of loop over steps

    event!(Level::DEBUG, "Pipeline execution completed successfully.");
    Ok(PipelineResult::Completed)
  }
}
