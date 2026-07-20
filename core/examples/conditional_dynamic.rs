use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use std::sync::Arc;
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct MainDynContext {
  trigger_value: i32,
  log: Vec<String>,
  shared_input_for_scoped: String,
}

#[derive(Clone, Debug, Default)]
struct DynScopedCtxAlpha {
  input: String,
  message_alpha: String,
}

#[derive(Clone, Debug, Default)]
struct DynScopedCtxBeta {
  input: String,
  message_beta: String,
  is_special_beta: bool,
}

type AppError = OrkaError;

// --- Factories for dynamic scoped pipelines ---
// A factory is handed the main context and builds the scoped pipeline on the fly, so it
// can specialise the pipeline to the current run. It may also fail with an `OrkaError`.

async fn factory_for_alpha(
  main_ctx: ContextData<MainDynContext>,
) -> Result<Arc<Pipeline<DynScopedCtxAlpha, AppError>>, OrkaError> {
  let main_trigger_val = main_ctx.read().trigger_value;
  info!(
    "Dynamic Factory Alpha: Creating pipeline. Main trigger value was: {}",
    main_trigger_val
  );

  if main_trigger_val == 42 {
    return Err(OrkaError::PipelineProviderFailure {
      step_name: "factory_for_alpha_init_fail".to_string(),
      source: anyhow::anyhow!("Factory Alpha cannot proceed with trigger_value 42"),
    });
  }

  let mut p_alpha = Pipeline::<DynScopedCtxAlpha, AppError>::new(["process_alpha_dyn"]);
  p_alpha.on_root("process_alpha_dyn", |s_ctx| async move {
    let mut data = s_ctx.write();
    data.message_alpha = format!("Alpha dynamically processed: '{}'", data.input);
    info!("Scoped: {}", data.message_alpha);
    if data.input == "FAIL_ALPHA_HANDLER" {
      return Err(OrkaError::Internal("Alpha scoped handler failed".to_string()));
    }
    Ok(PipelineControl::Continue)
  });
  Ok(Arc::new(p_alpha))
}

async fn factory_for_beta(
  main_ctx: ContextData<MainDynContext>,
) -> Result<Arc<Pipeline<DynScopedCtxBeta, AppError>>, OrkaError> {
  let main_trigger_val = main_ctx.read().trigger_value;
  info!(
    "Dynamic Factory Beta: Creating pipeline. Main trigger value was: {}",
    main_trigger_val
  );

  let mut p_beta = Pipeline::<DynScopedCtxBeta, AppError>::new(["process_beta_dyn"]);
  p_beta.on_root("process_beta_dyn", move |s_ctx| {
    // Baked into the handler by the factory, from the main context's state.
    let is_special_from_factory = main_trigger_val > 100;
    async move {
      let mut data = s_ctx.write();
      data.message_beta = format!("Beta dynamically processed: '{}'", data.input);
      data.is_special_beta = is_special_from_factory;
      info!("Scoped: {}, Special: {}", data.message_beta, data.is_special_beta);
      Ok(PipelineControl::Continue)
    }
  });
  Ok(Arc::new(p_beta))
}

async fn always_failing_factory(
  _main_ctx: ContextData<MainDynContext>,
) -> Result<Arc<Pipeline<DynScopedCtxAlpha, AppError>>, OrkaError> {
  info!("Always Failing Factory: Intentionally returning error.");
  Err(OrkaError::PipelineProviderFailure {
    step_name: "always_failing_factory".to_string(),
    source: anyhow::anyhow!("Provider error from always_failing_factory"),
  })
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Dynamic Conditional Logic Example ---");

  let mut main_pipeline =
    Pipeline::<MainDynContext, AppError>::new(["set_trigger", "dynamic_conditional_step", "final_check"]);

  main_pipeline.on_root("set_trigger", |ctx| async move {
    let mut data = ctx.write();
    let log_msg = format!("Main: Trigger value set to: {}", data.trigger_value);
    info!("{}", log_msg);
    data.log.push(log_msg);
    Ok(PipelineControl::Continue)
  });

  main_pipeline
    .conditional_scopes_for_step("dynamic_conditional_step")
    .add_dynamic_scope(factory_for_alpha, |main_ctx: ContextData<MainDynContext>| {
      let input = main_ctx.read().shared_input_for_scoped.clone();
      info!("Extractor for Alpha: Input will be '{}'", input);
      Ok(ContextData::new(DynScopedCtxAlpha {
        input,
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainDynContext>| {
      let val = main_ctx.read().trigger_value;
      val > 0 && val <= 50
    })
    .add_dynamic_scope(factory_for_beta, |main_ctx: ContextData<MainDynContext>| {
      let input = main_ctx.read().shared_input_for_scoped.clone();
      info!("Extractor for Beta: Input will be '{}'", input);
      Ok(ContextData::new(DynScopedCtxBeta {
        input,
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainDynContext>| main_ctx.read().trigger_value > 50)
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false);

  main_pipeline.on_root("final_check", |ctx| async move {
    let mut data = ctx.write();
    let log_msg = format!("Main: Final check. Current log: {:?}", data.log);
    info!("{}", log_msg);
    data.log.push(log_msg);
    Ok(PipelineControl::Continue)
  });

  // --- Alpha scope matches ---
  info!("\n--- Running Scenario for Dynamic Alpha ---");
  let ctx_alpha = ContextData::new(MainDynContext {
    trigger_value: 25,
    shared_input_for_scoped: "Hello Alpha".to_string(),
    ..Default::default()
  });
  let result_alpha = main_pipeline.run(ctx_alpha.clone()).await?;
  assert_eq!(result_alpha, PipelineResult::Completed);
  let final_alpha_log = ctx_alpha.read().log.clone();
  assert!(final_alpha_log.iter().any(|s| s.contains("Trigger value set to: 25")));
  info!("Alpha scenario log: {:?}", final_alpha_log);

  // --- Beta scope matches, and the factory marks it "special" ---
  info!("\n--- Running Scenario for Dynamic Beta (special) ---");
  let ctx_beta = ContextData::new(MainDynContext {
    trigger_value: 150,
    shared_input_for_scoped: "Hello Beta".to_string(),
    ..Default::default()
  });
  let result_beta = main_pipeline.run(ctx_beta.clone()).await?;
  assert_eq!(result_beta, PipelineResult::Completed);
  let final_beta_log = ctx_beta.read().log.clone();
  assert!(final_beta_log.iter().any(|s| s.contains("Trigger value set to: 150")));
  info!("Beta scenario log: {:?}", final_beta_log);

  // --- Alpha's condition matches, but the factory itself refuses to build ---
  info!("\n--- Running Scenario: Alpha Factory has internal failure ---");
  let ctx_alpha_factory_fail = ContextData::new(MainDynContext {
    trigger_value: 42,
    shared_input_for_scoped: "Input for Alpha factory failure".to_string(),
    ..Default::default()
  });
  let result_alpha_factory_fail = main_pipeline.run(ctx_alpha_factory_fail.clone()).await;
  assert!(
    result_alpha_factory_fail.is_err(),
    "Expected pipeline to fail due to Alpha factory's internal error"
  );
  if let Err(e) = &result_alpha_factory_fail {
    info!("Pipeline failed as expected due to Alpha factory internal error: {}", e);
    assert!(format!("{:?}", e).contains("Factory Alpha cannot proceed with trigger_value 42"));
  }

  // --- The scoped pipeline builds fine, but its handler fails ---
  info!("\n--- Running Scenario: Scoped Alpha Handler Fails ---");
  let ctx_alpha_handler_fail = ContextData::new(MainDynContext {
    trigger_value: 10,
    shared_input_for_scoped: "FAIL_ALPHA_HANDLER".to_string(),
    ..Default::default()
  });
  let result_alpha_handler_fail = main_pipeline.run(ctx_alpha_handler_fail.clone()).await;
  assert!(
    result_alpha_handler_fail.is_err(),
    "Expected pipeline to fail due to Alpha scoped handler error"
  );
  if let Err(e) = &result_alpha_handler_fail {
    info!("Pipeline failed as expected due to Alpha scoped handler error: {}", e);
    assert!(format!("{:?}", e).contains("Alpha scoped handler failed"));
  }

  // --- A separate pipeline whose only scope has a provider that always fails ---
  let mut fail_test_pipeline = Pipeline::<MainDynContext, AppError>::new([
    "set_trigger_fail_test",
    "dynamic_cond_step_prov_fail_test",
  ]);
  fail_test_pipeline.on_root("set_trigger_fail_test", |ctx| async move {
    let mut data = ctx.write();
    let log_msg = format!("FailTest Main: Trigger for provider fail: {}", data.trigger_value);
    info!("{}", log_msg);
    data.log.push(log_msg);
    Ok(PipelineControl::Continue)
  });
  fail_test_pipeline
    .conditional_scopes_for_step("dynamic_cond_step_prov_fail_test")
    .add_dynamic_scope(always_failing_factory, |main_ctx: ContextData<MainDynContext>| {
      let input = main_ctx.read().shared_input_for_scoped.clone();
      Ok(ContextData::new(DynScopedCtxAlpha {
        input,
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainDynContext>| main_ctx.read().trigger_value == 777)
    .if_no_scope_matches(PipelineControl::Stop)
    .finalize_conditional_step(false);

  info!("\n--- Running Scenario: Provider (Factory) Fails Externally ---");
  let ctx_provider_fail = ContextData::new(MainDynContext {
    trigger_value: 777,
    shared_input_for_scoped: "Input for always_failing_provider".to_string(),
    ..Default::default()
  });
  let result_provider_fail = fail_test_pipeline.run(ctx_provider_fail.clone()).await;

  assert!(
    result_provider_fail.is_err(),
    "Expected pipeline to fail due to provider error. Got: {:?}",
    result_provider_fail
  );
  if let Err(e) = result_provider_fail {
    info!("Pipeline failed as expected due to provider error: {}", e);
    match e {
      OrkaError::PipelineProviderFailure { ref source, .. } => {
        assert!(source.to_string().contains("Provider error from always_failing_factory"));
      }
      // The conditional scope may also wrap the provider's error rather than pass it through.
      OrkaError::Internal(ref s)
        if s.contains("conditional_scope_provider") && s.contains("Provider error from always_failing_factory") => {}
      other_err => panic!(
        "Unexpected error type for provider failure: {:?}, expected PipelineProviderFailure containing 'always_failing_factory'",
        other_err
      ),
    }
  }

  Ok(())
}
