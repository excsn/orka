use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use tracing::info;

#[derive(Clone, Debug, Default)]
struct BasicContext {
  message_log: Vec<String>,
  counter: i32,
}

// This example uses `OrkaError` directly as its handler error type. Real applications
// usually define their own error with `#[from] OrkaError`; see `error_handling.rs`.
#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

  info!("--- Basic Pipeline Example ---");

  let mut pipeline = Pipeline::<BasicContext, OrkaError>::new(["step_alpha", "step_beta", "step_gamma"]);

  pipeline
    .on_root("step_alpha", |ctx| async move {
      let mut data = ctx.write();
      data.counter += 1;
      let msg = format!("Alpha executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      Ok(PipelineControl::Continue)
    })
    .on_root("step_beta", |ctx| async move {
      let mut data = ctx.write();
      data.counter *= 2;
      let msg = format!("Beta executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      Ok(PipelineControl::Continue)
    })
    .on_root("step_gamma", |ctx| async move {
      let mut data = ctx.write();
      data.counter -= 1;
      let msg = format!("Gamma executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      Ok(PipelineControl::Continue)
    });

  let pipeline_context = ContextData::new(BasicContext {
    message_log: Vec::new(),
    counter: 5,
  });

  info!("Starting pipeline execution...");
  let result = pipeline.run(pipeline_context.clone()).await?;

  match result {
    PipelineResult::Completed => info!("Pipeline completed successfully!"),
    PipelineResult::Stopped => info!("Pipeline was stopped early."),
    other => info!("Pipeline ended as {:?}.", other),
  }

  let final_context_state = pipeline_context.read();
  info!("Final counter value: {}", final_context_state.counter);
  info!("Execution log:");
  for log_entry in &final_context_state.message_log {
    info!("- {}", log_entry);
  }

  // (5 + 1) * 2 - 1 = 11
  assert_eq!(final_context_state.counter, 11);
  assert_eq!(final_context_state.message_log.len(), 3);

  Ok(())
}
