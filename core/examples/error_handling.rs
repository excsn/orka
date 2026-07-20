use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use tracing::{error, info};

#[derive(Debug, thiserror::Error)]
enum ExampleAppError {
  #[error("A custom application error occurred: {0}")]
  CustomError(String),

  #[error("Orka framework error during pipeline execution: {0}")]
  OrkaFramework(#[from] OrkaError),
}

#[derive(Clone, Debug, Default)]
struct ErrorContext {
  processed_steps: Vec<String>,
}

#[tokio::main]
async fn main() {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Error Handling Example ---");

  info!("\nScenario 1: Handler returns a custom error");
  run_pipeline_with_handler_error().await;

  info!("\nScenario 2: Orka framework error (HandlerMissing)");
  run_pipeline_with_framework_error().await;
}

async fn run_pipeline_with_handler_error() {
  let mut pipeline =
    Pipeline::<ErrorContext, ExampleAppError>::new(["step_one", "step_two_fails", "step_three"]);

  pipeline
    .on_root("step_one", |ctx| async move {
      info!("Executing step_one");
      ctx.write().processed_steps.push("step_one".to_string());
      Ok(PipelineControl::Continue)
    })
    .on_root("step_two_fails", |ctx| async move {
      info!("Executing step_two_fails - this will error");
      ctx.write().processed_steps.push("step_two_fails".to_string());
      Err(ExampleAppError::CustomError(
        "Something went wrong in step_two!".to_string(),
      ))
    })
    .on_root("step_three", |ctx| async move {
      info!("Executing step_three (should not be reached)");
      ctx.write().processed_steps.push("step_three".to_string());
      Ok(PipelineControl::Continue)
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
  // `step_beta_no_handler` is required but has no handler, so the run fails with
  // `OrkaError::HandlerMissing` at the moment that step is reached — after the
  // preceding steps have already run and committed their context changes.
  let mut pipeline = Pipeline::<ErrorContext, ExampleAppError>::new([
    "step_alpha",
    "step_beta_no_handler",
    "step_gamma",
  ]);

  pipeline.on_root("step_alpha", |ctx| async move {
    info!("Executing step_alpha");
    ctx.write().processed_steps.push("step_alpha".to_string());
    Ok(PipelineControl::Continue)
  });

  let context = ContextData::new(ErrorContext::default());
  match pipeline.run(context.clone()).await {
    Ok(pipeline_result) => {
      error!(
        "Pipeline unexpectedly succeeded (framework error test): {:?}",
        pipeline_result
      );
    }
    Err(ExampleAppError::OrkaFramework(orka_err)) => {
      info!("Pipeline failed with framework error as expected: {:?}", orka_err);
      assert!(matches!(orka_err, OrkaError::HandlerMissing { step_name } if step_name == "step_beta_no_handler"));
    }
    Err(e) => error!("Unexpected error type: {:?}", e),
  }

  let final_ctx = context.read();
  info!(
    "Processed steps (framework error test): {:?}",
    final_ctx.processed_steps
  );
  assert_eq!(final_ctx.processed_steps, vec!["step_alpha"]);
}
