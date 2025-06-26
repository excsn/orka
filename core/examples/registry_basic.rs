// orka_core/examples/registry_basic.rs

use orka::{ContextData, Orka, OrkaError, OrkaResult, Pipeline, PipelineControl, PipelineResult};
use std::sync::Arc;
use tracing::{error, info};

// --- Contexts for different pipelines ---
#[derive(Clone, Debug, Default)]
struct UserWorkflowContext {
  user_id: String,
  action_log: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct ProductWorkflowContext {
  product_id: String,
  update_log: Vec<String>,
}

// --- Custom Error Type for this example ---
#[derive(Debug, thiserror::Error)]
enum RegistryExampleError {
  #[error("User Workflow Error: {0}")]
  UserError(String),
  #[error("Product Workflow Error: {0}")]
  ProductError(String),
  #[error("Orka Framework Error in Registry Example: {0}")]
  Orka(#[from] OrkaError), // To allow Orka errors to be converted
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  // Use Box<dyn Error> for main
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Orka Registry Basic Example ---");

  // 1. Create an Orka registry instance
  // The registry is generic over the ApplicationError type.
  let orka_registry = Arc::new(Orka::<RegistryExampleError>::new());

  // 2. Define and register Pipeline A (User Workflow)
  let mut user_pipeline = Pipeline::<UserWorkflowContext, RegistryExampleError>::new(&[
    ("validate_user", false, None),
    ("process_user_action", false, None),
  ]);
  user_pipeline.on_root("validate_user", |ctx: ContextData<UserWorkflowContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let msg = format!("User Validated: {}", data.user_id);
      info!("{}", msg);
      data.action_log.push(msg);
      if data.user_id.is_empty() {
        return Err(RegistryExampleError::UserError("User ID cannot be empty".to_string()));
      }
      Ok(PipelineControl::Continue)
    })
  });
  user_pipeline.on_root("process_user_action", |ctx: ContextData<UserWorkflowContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let msg = format!("User Action Processed for: {}", data.user_id);
      info!("{}", msg);
      data.action_log.push(msg);
      OrkaResult::<_>::Ok(PipelineControl::Continue)
    })
  });
  orka_registry.register_pipeline(user_pipeline);
  info!("UserWorkflowPipeline registered.");

  // 3. Define and register Pipeline B (Product Workflow)
  let mut product_pipeline = Pipeline::<ProductWorkflowContext, RegistryExampleError>::new(&[
    ("check_product_stock", false, None),
    ("update_product_details", false, None),
  ]);
  product_pipeline.on_root("check_product_stock", |ctx: ContextData<ProductWorkflowContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let msg = format!("Stock Checked for Product: {}", data.product_id);
      info!("{}", msg);
      data.update_log.push(msg);
      if data.product_id == "FAIL" {
        return Err(RegistryExampleError::ProductError("Product check failed".to_string()));
      }
      Ok(PipelineControl::Continue)
    })
  });
  product_pipeline.on_root("update_product_details", |ctx: ContextData<ProductWorkflowContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      let msg = format!("Details Updated for Product: {}", data.product_id);
      info!("{}", msg);
      data.update_log.push(msg);
      Ok::<_, RegistryExampleError>(PipelineControl::Continue)
    })
  });
  orka_registry.register_pipeline(product_pipeline);
  info!("ProductWorkflowPipeline registered.");

  // 4. Run User Workflow Pipeline via Registry
  info!("\n--- Running User Workflow ---");
  let user_ctx_data = UserWorkflowContext {
    user_id: "user123".to_string(),
    ..Default::default()
  };
  let user_context = ContextData::new(user_ctx_data);
  match orka_registry.run(user_context.clone()).await {
    Ok(PipelineResult::Completed) => {
      info!("User workflow completed successfully.");
      let final_user_ctx = user_context.read();
      assert_eq!(final_user_ctx.action_log.len(), 2);
      info!("User action log: {:?}", final_user_ctx.action_log);
    }
    Err(e) => error!("User workflow failed: {}", e),
    _ => info!("User workflow stopped."),
  }

  // 5. Run Product Workflow Pipeline via Registry
  info!("\n--- Running Product Workflow ---");
  let product_ctx_data = ProductWorkflowContext {
    product_id: "prod789".to_string(),
    ..Default::default()
  };
  let product_context = ContextData::new(product_ctx_data);
  match orka_registry.run(product_context.clone()).await {
    Ok(PipelineResult::Completed) => {
      info!("Product workflow completed successfully.");
      let final_product_ctx = product_context.read();
      assert_eq!(final_product_ctx.update_log.len(), 2);
      info!("Product update log: {:?}", final_product_ctx.update_log);
    }
    Err(e) => error!("Product workflow failed: {}", e),
    _ => info!("Product workflow stopped."),
  }

  // 6. Run Product Workflow that Fails
  info!("\n--- Running Failing Product Workflow ---");
  let failing_product_ctx_data = ProductWorkflowContext {
    product_id: "FAIL".to_string(), // This will cause an error in its pipeline
    ..Default::default()
  };
  let failing_product_context = ContextData::new(failing_product_ctx_data);
  match orka_registry.run(failing_product_context.clone()).await {
    Ok(_) => error!("Failing product workflow unexpectedly succeeded!"),
    Err(RegistryExampleError::ProductError(msg)) => {
      info!("Failing product workflow failed as expected: {}", msg);
      assert!(msg.contains("Product check failed"));
    }
    Err(e) => error!("Failing product workflow failed with unexpected error type: {}", e),
  }

  // 7. Attempt to run pipeline for an unregistered context type
  info!("\n--- Running Unregistered Workflow ---");
  #[derive(Clone, Default, Debug)]
  struct UnregisteredCtx {
    id: i32,
  };
  let unregistered_context = ContextData::new(UnregisteredCtx::default());
  match orka_registry.run(unregistered_context).await {
    Ok(_) => error!("Unregistered workflow unexpectedly succeeded!"),
    Err(RegistryExampleError::Orka(orka_error)) => {
      info!(
        "Unregistered workflow failed as expected with OrkaError: {:?}",
        orka_error
      );
      assert!(matches!(orka_error, OrkaError::ConfigurationError { .. }));
    }
    Err(e) => error!("Unregistered workflow failed with unexpected error: {}", e),
  }

  Ok(())
}
