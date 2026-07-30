use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use tracing::{error, info};

#[derive(Clone, Debug, Default)]
struct StopContext {
  log: Vec<String>,
  stop_signal_received: bool,
}

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Pipeline Stop Example ---");

  let mut pipeline = Pipeline::<StopContext, OrkaError>::new([
    "step_one_stop",
    "step_two_stop_action",
    "step_three_after_stop",
  ]);

  pipeline
    .on_root("step_one_stop", |ctx| async move {
      let msg = "Step One Executed.".to_string();
      info!("{}", msg);
      ctx.write().log.push(msg);
      Ok(PipelineControl::Continue)
    })
    .on_root("step_two_stop_action", |ctx| async move {
      let msg = "Step Two Executed - Issuing STOP.".to_string();
      info!("{}", msg);
      let mut data = ctx.write();
      data.log.push(msg);
      data.stop_signal_received = true;
      Ok(PipelineControl::Stop)
    })
    .on_root("step_three_after_stop", |ctx| async move {
      // Reaching this means the Stop above was not honoured.
      let msg = "Step Three Executed (SHOULD NOT HAPPEN).".to_string();
      error!("{}", msg);
      ctx.write().log.push(msg);
      Ok(PipelineControl::Continue)
    });

  let initial_context = ContextData::new(StopContext::default());

  info!("Starting pipeline execution (expecting stop)...");
  let result = pipeline.run(initial_context.clone()).await?;

  match result {
    PipelineResult::Completed => {
      error!("Pipeline completed, but was expected to stop!");
    }
    PipelineResult::Stopped => {
      info!("Pipeline stopped as expected.");
    }
    other => {
      error!("Pipeline ended as {:?}, but was expected to stop!", other);
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
