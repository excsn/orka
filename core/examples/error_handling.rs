// orka_core/examples/error_handling.rs

use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use tracing::{error, info};

// 1. Define a custom application error type
#[derive(Debug, thiserror::Error)]
enum ExampleAppError {
  #[error("A custom application error occurred: {0}")]
  CustomError(String),

  #[error("Orka framework error during pipeline execution: {0}")]
  OrkaFramework(#[from] OrkaError), // Allows OrkaError to be converted into ExampleAppError
}

// 2. Define Context Data
#[derive(Clone, Debug, Default)]
struct ErrorContext {
  processed_steps: Vec<String>,
  fail_in_step: Option<String>,
}

#[tokio::main]
async fn main() {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Error Handling Example ---");

  // Scenario 1: Handler returns a custom error
  info!("\nScenario 1: Handler returns a custom error");
  run_pipeline_with_handler_error().await;

  // Scenario 2: Framework error (HandlerMissing)
  info!("\nScenario 2: Orka framework error (HandlerMissing)");
  run_pipeline_with_framework_error().await;
}

async fn run_pipeline_with_handler_error() {
  let mut pipeline = Pipeline::<ErrorContext, ExampleAppError>::new(&[
    ("step_one", false, None),
    ("step_two_fails", false, None),
    ("step_three", false, None), // Should not run
  ]);

  pipeline.on_root("step_one", |ctx: ContextData<ErrorContext>| {
    Box::pin(async move {
      info!("Executing step_one");
      ctx.write().processed_steps.push("step_one".to_string());
      Ok::<_, ExampleAppError>(PipelineControl::Continue)
    })
  });

  pipeline.on_root("step_two_fails", |ctx: ContextData<ErrorContext>| {
    Box::pin(async move {
      info!("Executing step_two_fails - this will error");
      ctx.write().processed_steps.push("step_two_fails".to_string());
      Err(ExampleAppError::CustomError(
        "Something went wrong in step_two!".to_string(),
      ))
    })
  });

  pipeline.on_root("step_three", |ctx: ContextData<ErrorContext>| {
    Box::pin(async move {
      info!("Executing step_three (should not be reached)");
      ctx.write().processed_steps.push("step_three".to_string());
      Ok::<_, ExampleAppError>(PipelineControl::Continue)
    })
  });

  let context = ContextData::new(ErrorContext::default());
  match pipeline.run(context.clone()).await {
    Ok(pipeline_result) => {
      error!("Pipeline unexpectedly succeeded: {:?}", pipeline_result);
    }
    Err(e) => {
      info!("Pipeline failed as expected: {}", e);
      match e {
        ExampleAppError::CustomError(msg) => {
          assert!(msg.contains("Something went wrong in step_two!"));
        }
        _ => error!("Unexpected error type: {:?}", e),
      }
    }
  }
  let final_ctx = context.read();
  info!("Processed steps: {:?}", final_ctx.processed_steps);
  assert_eq!(final_ctx.processed_steps, vec!["step_one", "step_two_fails"]);
}

async fn run_pipeline_with_framework_error() {
  // Create a pipeline with a non-optional step that has no handler
  let pipeline = Pipeline::<ErrorContext, ExampleAppError>::new(&[
    ("step_alpha", false, None),           // Handler will be registered
    ("step_beta_no_handler", false, None), // Non-optional, no handler
    ("step_gamma", false, None),
  ]);

  // Only register handler for step_alpha
  let mut pipeline_mut = pipeline; // Shadow to make it mutable for registration
  pipeline_mut.on_root("step_alpha", |ctx: ContextData<ErrorContext>| {
    Box::pin(async move {
      info!("Executing step_alpha");
      ctx.write().processed_steps.push("step_alpha".to_string());
      Ok::<_, ExampleAppError>(PipelineControl::Continue)
    })
  });
  // No handler for "step_beta_no_handler"

  let context = ContextData::new(ErrorContext::default());
  match pipeline_mut.run(context.clone()).await {
    Ok(pipeline_result) => {
      error!(
        "Pipeline unexpectedly succeeded (framework error test): {:?}",
        pipeline_result
      );
    }
    Err(e) => {
      info!("Pipeline failed with framework error as expected: {}", e);
      match e {
        ExampleAppError::OrkaFramework(orka_err) => {
          info!("Wrapped OrkaError: {:?}", orka_err);
          assert!(matches!(orka_err, OrkaError::HandlerMissing { step_name } if step_name == "step_beta_no_handler"));
        }
        _ => error!("Unexpected error type: {:?}", e),
      }
    }
  }
  let final_ctx = context.read();
  info!(
    "Processed steps (framework error test): {:?}",
    final_ctx.processed_steps
  );
  // Only step_alpha should have run before the HandlerMissing error
  assert!(final_ctx.processed_steps.is_empty() || final_ctx.processed_steps == vec!["step_alpha"]);
  // Correction: Pipeline::run checks for missing handlers *before* executing any step's handlers for that step.
  // If step_beta_no_handler is encountered and is non-optional with no handlers, the error happens
  // *when that step is about to be processed*.
  // So, if step_alpha is before it and completes, its effects will be seen.
  // The current `Pipeline::run` iterates steps. When it gets to `step_beta_no_handler`,
  // it checks for handlers. If none and not optional, it errors.
  // The original code loops through steps. Before executing handlers for a step, it checks
  // if handlers exist. If step_alpha runs, its context changes are committed.
  // Then, when step_beta is processed, the lack of handlers causes failure.
  // So, if step_alpha is defined and has a handler, it *will* run.
  // If the pipeline definition starts with step_beta_no_handler, then processed_steps will be empty.
  // Let's adjust assertion: if step_alpha is first and has handler, it runs.
  // For this test, let's assume step_alpha is not registered to make it cleaner.
  // No, let's test that step_alpha runs. The error for step_beta will occur when pipeline.run processes step_beta.

  // Re-running with step_alpha to confirm it executes before failure on step_beta
  let mut pipeline_scenario2 = Pipeline::<ErrorContext, ExampleAppError>::new(&[
    ("step_alpha_s2", false, None),
    ("step_beta_no_handler_s2", false, None),
  ]);
  pipeline_scenario2.on_root("step_alpha_s2", |ctx: ContextData<ErrorContext>| {
    Box::pin(async move {
      ctx.write().processed_steps.push("step_alpha_s2".to_string());
      Ok::<_, ExampleAppError>(PipelineControl::Continue)
    })
  });

  let context_s2 = ContextData::new(ErrorContext::default());
  match pipeline_scenario2.run(context_s2.clone()).await {
    Err(ExampleAppError::OrkaFramework(OrkaError::HandlerMissing { step_name }))
      if step_name == "step_beta_no_handler_s2" =>
    {
      info!("Correctly caught HandlerMissing for step_beta_no_handler_s2");
    }
    Err(e) => error!("Expected HandlerMissing, got {:?}", e),
    Ok(_) => error!("Expected HandlerMissing, but pipeline completed"),
  }
  assert_eq!(context_s2.read().processed_steps, vec!["step_alpha_s2"]);
}
