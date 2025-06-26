// orka_core/examples/basic_pipeline.rs

use orka::{ContextData, OrkaError, OrkaResult, Pipeline, PipelineControl, PipelineResult};
use tracing::info;

// 1. Define the Context Data for the pipeline
#[derive(Clone, Debug, Default)]
struct BasicContext {
  message_log: Vec<String>,
  counter: i32,
}

// 2. Define an Error type for the pipeline
//    For simplicity, this example uses OrkaError directly for its handlers.
//    In real applications, you'd typically define a custom error:
//    #[derive(Debug, thiserror::Error)]
//    enum MyError { #[error("Orka: {0}")] Orka(#[from] OrkaError), /* ... */ }

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  // Initialize tracing (optional, for demonstration)
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

  info!("--- Basic Pipeline Example ---");

  // 3. Create a new pipeline definition
  // Pipeline<TData, Err> where Err must be From<OrkaError>
  let mut pipeline = Pipeline::<BasicContext, OrkaError>::new(&[
    ("step_alpha", false, None), // Step name, optional, skip_if
    ("step_beta", false, None),
    ("step_gamma", false, None),
  ]);

  // 4. Register handlers for the steps
  pipeline.on_root("step_alpha", |ctx: ContextData<BasicContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.counter += 1;
      let msg = format!("Alpha executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      OrkaResult::<_>::Ok(PipelineControl::Continue)
    })
  });

  pipeline.on_root("step_beta", |ctx: ContextData<BasicContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.counter *= 2;
      let msg = format!("Beta executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  pipeline.on_root("step_gamma", |ctx: ContextData<BasicContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.counter -= 1;
      let msg = format!("Gamma executed: counter = {}", data.counter);
      info!("{}", msg);
      data.message_log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // 5. Create an initial context
  let initial_context_data = BasicContext {
    message_log: Vec::new(),
    counter: 5, // Start counter at 5
  };
  let pipeline_context = ContextData::new(initial_context_data);

  // 6. Run the pipeline
  info!("Starting pipeline execution...");
  let result = pipeline.run(pipeline_context.clone()).await?; // Propagate OrkaError if any

  // 7. Inspect the results
  match result {
    PipelineResult::Completed => info!("Pipeline completed successfully!"),
    PipelineResult::Stopped => info!("Pipeline was stopped early."),
  }

  let final_context_state = pipeline_context.read();
  info!("Final counter value: {}", final_context_state.counter);
  info!("Execution log:");
  for log_entry in &final_context_state.message_log {
    info!("- {}", log_entry);
  }

  // Expected: (5+1)*2 - 1 = 11
  assert_eq!(final_context_state.counter, 11);
  assert_eq!(final_context_state.message_log.len(), 3);

  Ok(())
}
