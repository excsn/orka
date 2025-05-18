// orka_core/examples/pipeline_stop.rs

use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use tracing::{error, info};

// 1. Define Context Data
#[derive(Clone, Debug, Default)]
struct StopContext {
  log: Vec<String>,
  stop_signal_received: bool,
}

// 2. Define Error Type (using OrkaError for simplicity)
// type AppError = OrkaError; // Or your custom error

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Pipeline Stop Example ---");

  // 3. Create Pipeline Definition
  let mut pipeline = Pipeline::<StopContext, OrkaError>::new(&[
    ("step_one_stop", false, None),
    ("step_two_stop_action", false, None),  // This step will issue a stop
    ("step_three_after_stop", false, None), // This step should not execute
  ]);

  // 4. Register Handlers
  pipeline.on_root("step_one_stop", |ctx: ContextData<StopContext>| {
    Box::pin(async move {
      let msg = "Step One Executed.".to_string();
      info!("{}", msg);
      ctx.write().log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  pipeline.on_root("step_two_stop_action", |ctx: ContextData<StopContext>| {
    Box::pin(async move {
      let msg = "Step Two Executed - Issuing STOP.".to_string();
      info!("{}", msg);
      let mut data = ctx.write();
      data.log.push(msg);
      data.stop_signal_received = true;
      Ok::<_, OrkaError>(PipelineControl::Stop) // Signal to stop the pipeline
    })
  });

  pipeline.on_root("step_three_after_stop", |ctx: ContextData<StopContext>| {
    Box::pin(async move {
      // This handler should not be reached
      let msg = "Step Three Executed (SHOULD NOT HAPPEN).".to_string();
      error!("{}", msg); // Use error level to highlight if it runs
      ctx.write().log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // 5. Create Initial Context
  let initial_context = ContextData::new(StopContext::default());

  // 6. Run the Pipeline
  info!("Starting pipeline execution (expecting stop)...");
  let result = pipeline.run(initial_context.clone()).await?;

  // 7. Inspect Results
  match result {
    PipelineResult::Completed => {
      error!("Pipeline completed, but was expected to stop!");
    }
    PipelineResult::Stopped => {
      info!("Pipeline stopped as expected.");
    }
  }

  let final_state = initial_context.read();
  info!("Execution Log:");
  for entry in &final_state.log {
    info!("- {}", entry);
  }
  assert!(final_state.stop_signal_received, "Stop signal was not processed.");
  assert_eq!(final_state.log.len(), 2, "Incorrect number of steps executed.");
  assert!(
    !final_state.log.iter().any(|s| s.contains("Step Three")),
    "Step after stop signal was unexpectedly executed."
  );

  Ok(())
}
