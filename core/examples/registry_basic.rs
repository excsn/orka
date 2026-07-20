use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl, PipelineResult};
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

#[derive(Debug, thiserror::Error)]
enum RegistryExampleError {
  #[error("User Workflow Error: {0}")]
  UserError(String),
  #[error("Product Workflow Error: {0}")]
  ProductError(String),
  #[error("Orka Framework Error in Registry Example: {0}")]
  Orka(#[from] OrkaError),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Orka Registry Basic Example ---");

  // The registry is generic over the application error type; each pipeline it holds is
  // keyed by its context type.
  let orka_registry = Arc::new(Orka::<RegistryExampleError>::new());

  let mut user_pipeline =
    Pipeline::<UserWorkflowContext, RegistryExampleError>::new(["validate_user", "process_user_action"]);
  user_pipeline
    .on_root("validate_user", |ctx| async move {
      let mut data = ctx.write();
      let msg = format!("User Validated: {}", data.user_id);
      info!("{}", msg);
      data.action_log.push(msg);
      if data.user_id.is_empty() {
        return Err(RegistryExampleError::UserError("User ID cannot be empty".to_string()));
      }
      Ok(PipelineControl::Continue)
    })
    .on_root("process_user_action", |ctx| async move {
      let mut data = ctx.write();
      let msg = format!("User Action Processed for: {}", data.user_id);
      info!("{}", msg);
      data.action_log.push(msg);
      Ok(PipelineControl::Continue)
    });
  orka_registry.register_pipeline(user_pipeline)?;
  info!("UserWorkflowPipeline registered.");

  let mut product_pipeline = Pipeline::<ProductWorkflowContext, RegistryExampleError>::new([
    "check_product_stock",
    "update_product_details",
  ]);
  product_pipeline
    .on_root("check_product_stock", |ctx| async move {
      let mut data = ctx.write();
      let msg = format!("Stock Checked for Product: {}", data.product_id);
      info!("{}", msg);
      data.update_log.push(msg);
      if data.product_id == "FAIL" {
        return Err(RegistryExampleError::ProductError("Product check failed".to_string()));
      }
      Ok(PipelineControl::Continue)
    })
    .on_root("update_product_details", |ctx| async move {
      let mut data = ctx.write();
      let msg = format!("Details Updated for Product: {}", data.product_id);
      info!("{}", msg);
      data.update_log.push(msg);
      Ok(PipelineControl::Continue)
    });
  orka_registry.register_pipeline(product_pipeline)?;
  info!("ProductWorkflowPipeline registered.");

  info!("\n--- Running User Workflow ---");
  let user_context = ContextData::new(UserWorkflowContext {
    user_id: "user123".to_string(),
    ..Default::default()
  });
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

  info!("\n--- Running Product Workflow ---");
  let product_context = ContextData::new(ProductWorkflowContext {
    product_id: "prod789".to_string(),
    ..Default::default()
  });
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

  info!("\n--- Running Failing Product Workflow ---");
  let failing_product_context = ContextData::new(ProductWorkflowContext {
    product_id: "FAIL".to_string(),
    ..Default::default()
  });
  match orka_registry.run(failing_product_context.clone()).await {
    Ok(_) => error!("Failing product workflow unexpectedly succeeded!"),
    Err(RegistryExampleError::ProductError(msg)) => {
      info!("Failing product workflow failed as expected: {}", msg);
      assert!(msg.contains("Product check failed"));
    }
    Err(e) => error!("Failing product workflow failed with unexpected error type: {}", e),
  }

  info!("\n--- Running Unregistered Workflow ---");
  #[derive(Clone, Default, Debug)]
  struct UnregisteredCtx;
  let unregistered_context = ContextData::new(UnregisteredCtx);
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
