// orka_core/examples/conditional_dynamic.rs

use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use std::future::{self, Future}; // Added future::ready
use std::pin::Pin;
use std::sync::Arc;
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct MainDynContext {
  trigger_value: i32,
  log: Vec<String>, // For logging main pipeline actions
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

// --- Error Type (Using OrkaError for simplicity in this example) ---
// In a real app, this would be a custom error type that implements From<OrkaError>
type AppError = OrkaError;

// --- Factory Functions for Dynamic Scoped Pipelines ---
// Factories return Future<Output = Result<Arc<Pipeline<SData, AppError>>, OrkaError>>
// Here, AppError is OrkaError, so it's Result<Arc<Pipeline<SData, OrkaError>>, OrkaError>

fn factory_for_alpha(
  main_ctx: ContextData<MainDynContext>,
) -> Pin<Box<dyn Future<Output = Result<Arc<Pipeline<DynScopedCtxAlpha, AppError>>, OrkaError>> + Send>> {
  let main_trigger_val = main_ctx.read().trigger_value;
  Box::pin(async move {
    info!(
      "Dynamic Factory Alpha: Creating pipeline. Main trigger value was: {}",
      main_trigger_val
    );
    // This factory could itself fail based on main_trigger_val or other conditions
    if main_trigger_val == 42 {
      // Arbitrary factory-level failure condition
      return Err(OrkaError::PipelineProviderFailure {
        step_name: "factory_for_alpha_init_fail".to_string(),
        source: anyhow::anyhow!("Factory Alpha cannot proceed with trigger_value 42"),
      });
    }

    let mut p_alpha = Pipeline::<DynScopedCtxAlpha, AppError>::new(&[("process_alpha_dyn", false, None)]);
    p_alpha.on_root("process_alpha_dyn", |s_ctx: ContextData<DynScopedCtxAlpha>| {
      Box::pin(async move {
        let mut data = s_ctx.write();
        data.message_alpha = format!("Alpha dynamically processed: '{}'", data.input);
        info!("Scoped: {}", data.message_alpha);
        // Example of scoped pipeline error
        if data.input == "FAIL_ALPHA_HANDLER" {
          return Err(OrkaError::Internal("Alpha scoped handler failed".to_string()));
        }
        Ok(PipelineControl::Continue)
      })
    });
    Ok(Arc::new(p_alpha))
  })
}

fn factory_for_beta(
  main_ctx: ContextData<MainDynContext>,
) -> Pin<Box<dyn Future<Output = Result<Arc<Pipeline<DynScopedCtxBeta, AppError>>, OrkaError>> + Send>> {
  let main_trigger_val = main_ctx.read().trigger_value;
  Box::pin(async move {
    info!(
      "Dynamic Factory Beta: Creating pipeline. Main trigger value was: {}",
      main_trigger_val
    );
    let mut p_beta = Pipeline::<DynScopedCtxBeta, AppError>::new(&[("process_beta_dyn", false, None)]);
    p_beta.on_root("process_beta_dyn", move |s_ctx: ContextData<DynScopedCtxBeta>| {
      let is_special_from_factory = main_trigger_val > 100;
      Box::pin(async move {
        let mut data = s_ctx.write();
        data.message_beta = format!("Beta dynamically processed: '{}'", data.input);
        data.is_special_beta = is_special_from_factory;
        info!("Scoped: {}, Special: {}", data.message_beta, data.is_special_beta);
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    });
    Ok(Arc::new(p_beta))
  })
}

// --- Factory that always fails (for testing provider failure) ---
fn always_failing_factory(
  _main_ctx: ContextData<MainDynContext>,
) -> Pin<Box<dyn Future<Output = Result<Arc<Pipeline<DynScopedCtxAlpha, AppError>>, OrkaError>> + Send>> {
  Box::pin(async move {
    info!("Always Failing Factory: Intentionally returning error.");
    Err(OrkaError::PipelineProviderFailure {
      step_name: "always_failing_factory".to_string(),
      source: anyhow::anyhow!("Provider error from always_failing_factory"),
    })
  })
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
  // Main returns AppError (which is OrkaError here)
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Dynamic Conditional Logic Example ---");

  // --- Main Pipeline Definition ---
  let mut main_pipeline = Pipeline::<MainDynContext, AppError>::new(&[
    ("set_trigger", false, None),
    ("dynamic_conditional_step", false, None),
    ("final_check", false, None),
  ]);

  main_pipeline.on_root("set_trigger", |ctx: ContextData<MainDynContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let log_msg = format!("Main: Trigger value set to: {}", data.trigger_value);
      info!("{}", log_msg);
      data.log.push(log_msg);
      Ok::<_, AppError>(PipelineControl::Continue)
    })
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
      val > 0 && val <= 50 // Condition to run Alpha
    })
    .add_dynamic_scope(factory_for_beta, |main_ctx: ContextData<MainDynContext>| {
      let input = main_ctx.read().shared_input_for_scoped.clone();
      info!("Extractor for Beta: Input will be '{}'", input);
      Ok(ContextData::new(DynScopedCtxBeta {
        input,
        ..Default::default()
      }))
    })
    .on_condition(|main_ctx: ContextData<MainDynContext>| {
      let val = main_ctx.read().trigger_value;
      val > 50 // Condition to run Beta
    })
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false); // This conditional step is not optional

  main_pipeline.on_root("final_check", |ctx: ContextData<MainDynContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let log_msg = format!("Main: Final check. Current log: {:?}", data.log);
      info!("{}", log_msg);
      data.log.push(log_msg);
      Ok::<_, AppError>(PipelineControl::Continue)
    })
  });

  // --- Run Scenario for Alpha ---
  info!("\n--- Running Scenario for Dynamic Alpha ---");
  let initial_alpha = MainDynContext {
    trigger_value: 25, // Meets Alpha condition
    shared_input_for_scoped: "Hello Alpha".to_string(),
    ..Default::default()
  };
  let ctx_alpha = ContextData::new(initial_alpha);
  let result_alpha = main_pipeline.run(ctx_alpha.clone()).await?;
  assert_eq!(result_alpha, PipelineResult::Completed);
  let final_alpha_log = ctx_alpha.read().log.clone();
  assert!(final_alpha_log.iter().any(|s| s.contains("Trigger value set to: 25")));
  // Check logs from scoped pipeline via tracing output for this simple example.
  // A more robust check would be if scoped pipeline modified shared_input_for_scoped or another field.
  info!("Alpha scenario log: {:?}", final_alpha_log);

  // --- Run Scenario for Beta ---
  info!("\n--- Running Scenario for Dynamic Beta (special) ---");
  let initial_beta = MainDynContext {
    trigger_value: 150, // Meets Beta condition and makes it special
    shared_input_for_scoped: "Hello Beta".to_string(),
    ..Default::default()
  };
  let ctx_beta = ContextData::new(initial_beta);
  let result_beta = main_pipeline.run(ctx_beta.clone()).await?;
  assert_eq!(result_beta, PipelineResult::Completed);
  let final_beta_log = ctx_beta.read().log.clone();
  assert!(final_beta_log.iter().any(|s| s.contains("Trigger value set to: 150")));
  info!("Beta scenario log: {:?}", final_beta_log);

  // --- Run Scenario: Factory for Alpha itself fails (not due to condition, but internal factory logic) ---
  info!("\n--- Running Scenario: Alpha Factory has internal failure ---");
  let initial_alpha_factory_fail = MainDynContext {
    trigger_value: 42, // Meets Alpha condition, but factory_for_alpha has specific logic to fail on 42
    shared_input_for_scoped: "Input for Alpha factory failure".to_string(),
    ..Default::default()
  };
  let ctx_alpha_factory_fail = ContextData::new(initial_alpha_factory_fail);
  let result_alpha_factory_fail = main_pipeline.run(ctx_alpha_factory_fail.clone()).await;
  assert!(
    result_alpha_factory_fail.is_err(),
    "Expected pipeline to fail due to Alpha factory's internal error"
  );
  if let Err(e) = &result_alpha_factory_fail {
    info!("Pipeline failed as expected due to Alpha factory internal error: {}", e);
    assert!(format!("{:?}", e).contains("Factory Alpha cannot proceed with trigger_value 42"));
  }

  // --- Run Scenario: Scoped Alpha Handler Fails ---
  info!("\n--- Running Scenario: Scoped Alpha Handler Fails ---");
  let initial_alpha_handler_fail = MainDynContext {
    trigger_value: 10,                                         // Meets Alpha condition
    shared_input_for_scoped: "FAIL_ALPHA_HANDLER".to_string(), // This will make the scoped handler fail
    ..Default::default()
  };
  let ctx_alpha_handler_fail = ContextData::new(initial_alpha_handler_fail);
  let result_alpha_handler_fail = main_pipeline.run(ctx_alpha_handler_fail.clone()).await;
  assert!(
    result_alpha_handler_fail.is_err(),
    "Expected pipeline to fail due to Alpha scoped handler error"
  );
  if let Err(e) = &result_alpha_handler_fail {
    info!("Pipeline failed as expected due to Alpha scoped handler error: {}", e);
    assert!(format!("{:?}", e).contains("Alpha scoped handler failed"));
  }

  // --- Setup and Run Pipeline for Provider Failure Test ---
  let mut fail_test_pipeline = Pipeline::<MainDynContext, AppError>::new(&[
    ("set_trigger_fail_test", false, None),
    ("dynamic_cond_step_prov_fail_test", false, None), // Note: renamed step for clarity
  ]);
  fail_test_pipeline.on_root("set_trigger_fail_test", |ctx: ContextData<MainDynContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let log_msg = format!("FailTest Main: Trigger for provider fail: {}", data.trigger_value);
      info!("{}", log_msg);
      data.log.push(log_msg);
      Ok::<_, AppError>(PipelineControl::Continue)
    })
  });
  fail_test_pipeline
    .conditional_scopes_for_step("dynamic_cond_step_prov_fail_test")
    .add_dynamic_scope(
      always_failing_factory, // Use the factory that is designed to fail
      |main_ctx: ContextData<MainDynContext>| {
        let input = main_ctx.read().shared_input_for_scoped.clone();
        Ok(ContextData::new(DynScopedCtxAlpha {
          input,
          ..Default::default()
        }))
      },
    )
    .on_condition(|main_ctx: ContextData<MainDynContext>| {
      main_ctx.read().trigger_value == 777 // Condition to trigger this specific scope
    })
    .if_no_scope_matches(PipelineControl::Stop) // If this scope isn't hit (e.g. wrong trigger), stop.
    .finalize_conditional_step(false);

  info!("\n--- Running Scenario: Provider (Factory) Fails Externally ---");
  let initial_provider_fail = MainDynContext {
    trigger_value: 777, // This will satisfy the condition for always_failing_factory
    shared_input_for_scoped: "Input for always_failing_provider".to_string(),
    ..Default::default()
  };
  let ctx_provider_fail = ContextData::new(initial_provider_fail);
  let result_provider_fail = fail_test_pipeline.run(ctx_provider_fail.clone()).await;

  assert!(
    result_provider_fail.is_err(),
    "Expected pipeline to fail due to provider error. Got: {:?}",
    result_provider_fail
  );
  if let Err(e) = result_provider_fail {
    info!("Pipeline failed as expected due to provider error: {}", e);
    match e {
            // After redesign, AnyConditionalScope converts provider's OrkaError to MainErr.
            // Since MainErr is OrkaError here, it should be the direct OrkaError.
            OrkaError::PipelineProviderFailure { ref source, .. } => {
                assert!(source.to_string().contains("Provider error from always_failing_factory"));
            }
            // It might also be wrapped by the AnyConditionalScope's error enrichment
            OrkaError::Internal(s) if s.contains("conditional_scope_provider") && s.contains("Provider error from always_failing_factory") => {
                // This is also acceptable if AnyConditionalScope wraps it.
            }
            other_err => panic!("Unexpected error type for provider failure: {:?}, expected PipelineProviderFailure containing 'always_failing_factory'", other_err),
        }
  }

  Ok(())
}
