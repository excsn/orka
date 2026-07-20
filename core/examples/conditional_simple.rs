use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use std::sync::Arc;
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct MainCondContext {
  condition_flag: String,
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

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Simple Conditional Logic Example ---");

  let mut scoped_pipeline_a = Pipeline::<ScopedACtx, OrkaError>::new(["task_a"]);
  scoped_pipeline_a.on_root("task_a", |ctx| async move {
    let mut data = ctx.write();
    data.processed_by_a = true;
    info!("Scoped Pipeline A executed with input: '{}'", data.input_data);
    Ok(PipelineControl::Continue)
  });
  let arc_scoped_a = Arc::new(scoped_pipeline_a);

  let mut scoped_pipeline_b = Pipeline::<ScopedBCtx, OrkaError>::new(["task_b"]);
  scoped_pipeline_b.on_root("task_b", |ctx| async move {
    let mut data = ctx.write();
    data.processed_by_b = true;
    info!("Scoped Pipeline B executed with input: '{}'", data.input_data);
    Ok(PipelineControl::Continue)
  });
  let arc_scoped_b = Arc::new(scoped_pipeline_b);

  let mut main_pipeline = Pipeline::<MainCondContext, OrkaError>::new([
    "setup_condition",
    "conditional_dispatch",
    "verify_after_dispatch",
  ]);

  main_pipeline.on_root("setup_condition", |ctx| async move {
    let mut data = ctx.write();
    let condition_flag = data.condition_flag.clone();
    info!("Main: Setup complete. Condition: '{}'", condition_flag);
    data
      .log
      .push(format!("Setup complete. Condition: '{}'", condition_flag));
    Ok(PipelineControl::Continue)
  });

  main_pipeline
    .conditional_scopes_for_step("conditional_dispatch")
    .add_static_scope(arc_scoped_a.clone(), |main_ctx: ContextData<MainCondContext>| {
      let data = main_ctx.read();
      info!(
        "Extractor for A: main_ctx.data_for_scoped_pipeline = '{}'",
        data.data_for_scoped_pipeline
      );
      Ok(ContextData::new(ScopedACtx {
        input_data: data.data_for_scoped_pipeline.clone(),
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainCondContext>| main_ctx.read().condition_flag == "A")
    .add_static_scope(arc_scoped_b.clone(), |main_ctx: ContextData<MainCondContext>| {
      let data = main_ctx.read();
      info!(
        "Extractor for B: main_ctx.data_for_scoped_pipeline = '{}'",
        data.data_for_scoped_pipeline
      );
      Ok(ContextData::new(ScopedBCtx {
        input_data: data.data_for_scoped_pipeline.clone(),
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainCondContext>| main_ctx.read().condition_flag == "B")
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false);

  main_pipeline.on_root("verify_after_dispatch", |ctx| async move {
    let data = ctx.read();
    info!("Main: Verification step. Log size: {}", data.log.len());
    Ok(PipelineControl::Continue)
  });

  // --- Scenario A: scope A's condition matches ---
  info!("\n--- Running Scenario A ---");
  let pipeline_context_a = ContextData::new(MainCondContext {
    condition_flag: "A".to_string(),
    data_for_scoped_pipeline: "Data for A".to_string(),
    ..Default::default()
  });
  let result_a = main_pipeline.run(pipeline_context_a.clone()).await?;
  assert_eq!(result_a, PipelineResult::Completed);
  {
    // Scope the guard: it must not still be held when the next scenario awaits.
    let final_a = pipeline_context_a.read();
    info!("Final log for A: {:?}", final_a.log);
    assert!(final_a.log.join(" ").contains("Setup complete. Condition: 'A'"));
  }

  // --- Scenario B: scope B's condition matches ---
  info!("\n--- Running Scenario B ---");
  let pipeline_context_b = ContextData::new(MainCondContext {
    condition_flag: "B".to_string(),
    data_for_scoped_pipeline: "Data for B".to_string(),
    ..Default::default()
  });
  let result_b = main_pipeline.run(pipeline_context_b.clone()).await?;
  assert_eq!(result_b, PipelineResult::Completed);
  {
    // Scope the guard: it must not still be held when the next scenario awaits.
    let final_b = pipeline_context_b.read();
    info!("Final log for B: {:?}", final_b.log);
    assert!(final_b.log.join(" ").contains("Setup complete. Condition: 'B'"));
  }

  // --- Scenario C: no scope matches, so `if_no_scope_matches` decides ---
  info!("\n--- Running Scenario No Match ---");
  let pipeline_context_none = ContextData::new(MainCondContext {
    condition_flag: "C".to_string(),
    data_for_scoped_pipeline: "Data for None".to_string(),
    ..Default::default()
  });
  let result_none = main_pipeline.run(pipeline_context_none.clone()).await?;
  assert_eq!(result_none, PipelineResult::Completed);
  {
    let final_none = pipeline_context_none.read();
    info!("Final log for No Match: {:?}", final_none.log);
    assert!(final_none.log.join(" ").contains("Setup complete. Condition: 'C'"));
  }

  Ok(())
}
