// orka_core/examples/conditional_simple.rs

use orka::{
  ContextData,
  OrkaError,
  Pipeline,
  PipelineControl,
  PipelineResult,
  StaticPipelineProvider, // For add_static_scope
};
use std::sync::Arc;
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct MainCondContext {
  condition_flag: String, // "A" or "B" or something else
  log: Vec<String>,
  data_for_scoped_pipeline: String,
}

#[derive(Clone, Debug, Default)]
struct ScopedACtx {
  input_data: String,
  processed_by_a: bool,
}

#[derive(Clone, Debug, Default)]
struct ScopedBCtx {
  input_data: String,
  processed_by_b: bool,
}

// --- Error Type --- (Using OrkaError for simplicity in example)
// type AppError = OrkaError;

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Simple Conditional Logic Example ---");

  // --- Define Scoped Pipelines (Statically) ---
  // Scoped Pipeline A
  let mut scoped_pipeline_a = Pipeline::<ScopedACtx, OrkaError>::new(&[("task_a", false, None)]);
  scoped_pipeline_a.on_root("task_a", |ctx: ContextData<ScopedACtx>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.processed_by_a = true;
      let msg = format!("Scoped Pipeline A executed with input: '{}'", data.input_data);
      info!("{}", msg);
      // To see effect in main_ctx, main_ctx.log would need to be updated
      // by the extractor or an after_root handler on main_ctx for the conditional step.
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });
  let arc_scoped_a = Arc::new(scoped_pipeline_a);

  // Scoped Pipeline B
  let mut scoped_pipeline_b = Pipeline::<ScopedBCtx, OrkaError>::new(&[("task_b", false, None)]);
  scoped_pipeline_b.on_root("task_b", |ctx: ContextData<ScopedBCtx>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.processed_by_b = true;
      let msg = format!("Scoped Pipeline B executed with input: '{}'", data.input_data);
      info!("{}", msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });
  let arc_scoped_b = Arc::new(scoped_pipeline_b);

  // --- Define Main Pipeline ---
  let mut main_pipeline = Pipeline::<MainCondContext, OrkaError>::new(&[
    ("setup_condition", false, None),
    ("conditional_dispatch", false, None),
    ("verify_after_dispatch", false, None),
  ]);

  main_pipeline.on_root("setup_condition", |ctx: ContextData<MainCondContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let condition_flag = data.condition_flag.clone();
      // data.condition_flag will be set by the test runner
      data
        .log
        .push(format!("Setup complete. Condition: '{}'", condition_flag));
      info!("Main: Setup complete. Condition: '{}'", data.condition_flag);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // Configure the conditional step
  main_pipeline
    .conditional_scopes_for_step("conditional_dispatch")
    .add_static_scope(
      arc_scoped_a.clone(), // Provide Arc<Pipeline<ScopedACtx, OrkaError>>
      // Extractor for ScopedACtx from MainCondContext
      // Returns Result<ContextData<SData>, OrkaError>
      |main_ctx: ContextData<MainCondContext>| {
        let data = main_ctx.read();
        info!(
          "Extractor for A: main_ctx.data_for_scoped_pipeline = '{}'",
          data.data_for_scoped_pipeline
        );
        Ok(ContextData::new(ScopedACtx {
          input_data: data.data_for_scoped_pipeline.clone(),
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<MainCondContext>| main_ctx.read().condition_flag == "A")
    .add_static_scope(
      arc_scoped_b.clone(), // Provide Arc<Pipeline<ScopedBCtx, OrkaError>>
      // Extractor for ScopedBCtx from MainCondContext
      |main_ctx: ContextData<MainCondContext>| {
        let data = main_ctx.read();
        info!(
          "Extractor for B: main_ctx.data_for_scoped_pipeline = '{}'",
          data.data_for_scoped_pipeline
        );
        Ok(ContextData::new(ScopedBCtx {
          input_data: data.data_for_scoped_pipeline.clone(),
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<MainCondContext>| main_ctx.read().condition_flag == "B")
    .if_no_scope_matches(PipelineControl::Continue) // What to do if no conditions match
    .finalize_conditional_step(false); // The conditional step itself is not optional

  main_pipeline.on_root("verify_after_dispatch", |ctx: ContextData<MainCondContext>| {
    Box::pin(async move {
      let data = ctx.read();
      let msg = format!("Main: Verification step. Log size: {}", data.log.len());
      info!("{}", msg);
      data.log.clone(); // To use data in assertion after drop
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // --- Run Scenario A ---
  info!("\n--- Running Scenario A ---");
  let initial_context_a = MainCondContext {
    condition_flag: "A".to_string(),
    data_for_scoped_pipeline: "Data for A".to_string(),
    ..Default::default()
  };
  let pipeline_context_a = ContextData::new(initial_context_a);
  let result_a = main_pipeline.run(pipeline_context_a.clone()).await?;
  assert_eq!(result_a, PipelineResult::Completed);
  let final_a = pipeline_context_a.read();
  info!("Final log for A: {:?}", final_a.log);
  // To assert scoped pipeline A ran, we'd need side effects visible here or use counters.
  // For this example, logs show execution. We expect 2 log entries in main if scoped did not log to main_ctx.
  // "Setup complete..." and "Main: Verification step..."
  // Let's add a log in the main pipeline after conditional dispatch to confirm.
  assert!(final_a.log.join(" ").contains("Setup complete. Condition: 'A'"));

  // --- Run Scenario B ---
  info!("\n--- Running Scenario B ---");
  let initial_context_b = MainCondContext {
    condition_flag: "B".to_string(),
    data_for_scoped_pipeline: "Data for B".to_string(),
    ..Default::default()
  };
  let pipeline_context_b = ContextData::new(initial_context_b);
  let result_b = main_pipeline.run(pipeline_context_b.clone()).await?;
  assert_eq!(result_b, PipelineResult::Completed);
  let final_b = pipeline_context_b.read();
  info!("Final log for B: {:?}", final_b.log);
  assert!(final_b.log.join(" ").contains("Setup complete. Condition: 'B'"));

  // --- Run Scenario No Match ---
  info!("\n--- Running Scenario No Match ---");
  let initial_context_none = MainCondContext {
    condition_flag: "C".to_string(), // No scope matches this
    data_for_scoped_pipeline: "Data for None".to_string(),
    ..Default::default()
  };
  let pipeline_context_none = ContextData::new(initial_context_none);
  let result_none = main_pipeline.run(pipeline_context_none.clone()).await?;
  assert_eq!(result_none, PipelineResult::Completed); // Because if_no_scope_matches is Continue
  let final_none = pipeline_context_none.read();
  info!("Final log for No Match: {:?}", final_none.log);
  assert!(final_none.log.join(" ").contains("Setup complete. Condition: 'C'"));

  Ok(())
}
