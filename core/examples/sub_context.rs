// orka_core/examples/sub_context.rs

use orka::{ContextData, ContextDataExtractorImpl, OrkaError, Pipeline, PipelineControl, PipelineResult};
use std::sync::Arc;
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct OrderProcessContext {
  // TData
  order_id: String,
  customer_details: CustomerInfo, // This will be SData
  shipping_details: ShippingInfo, // Another part of TData
  is_processed: bool,
  log: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct CustomerInfo {
  // SData
  customer_id: String,
  name: String,
  email: String,
  is_validated: bool,
}

#[derive(Clone, Debug, Default)]
struct ShippingInfo {
  address: String,
  is_confirmed: bool,
}

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Sub-Context Extraction Example (Non-Conditional) ---");

  // Define the pipeline once
  let mut pipeline = Pipeline::<OrderProcessContext, OrkaError>::new(&[
    ("initialize_order", false, None),
    ("process_customer_info", false, None),
    ("process_shipping", false, None),
    ("finalize_order", false, None),
  ]);

  // Initialize order handler (this will run for both scenarios)
  pipeline.on_root("initialize_order", |ctx: ContextData<OrderProcessContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      // Only initialize if order_id isn't already set (to allow test setup)
      if data.order_id.is_empty() {
        data.order_id = "ORD123".to_string();
        data.customer_details = CustomerInfo {
          customer_id: "CUST456".to_string(),
          name: "John Doe".to_string(),
          email: "john.doe@example.com".to_string(), // Default valid email
          is_validated: false,
        };
        data.shipping_details = ShippingInfo {
          address: "123 Main St".to_string(),
          is_confirmed: false,
        };
      }
      let msg = format!(
        "Order {} initialized/checked for customer {}",
        data.order_id, data.customer_details.customer_id
      );
      info!("{}", msg);
      data.log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // Extractor and on<SData> handlers for process_customer_info (same as before)
  pipeline.set_extractor("process_customer_info", |main_ctx: ContextData<OrderProcessContext>| {
    let customer_info_clone = main_ctx.read().customer_details.clone();
    info!(
      "Extractor: Extracting CustomerInfo for order {}",
      main_ctx.read().order_id
    );
    Ok(ContextData::new(customer_info_clone))
  });
  pipeline.on::<CustomerInfo, _, OrkaError>("process_customer_info", |s_ctx: ContextData<CustomerInfo>| {
    Box::pin(async move {
      let mut cust_info = s_ctx.write();
      info!("Sub-Handler: Processing customer {}", cust_info.customer_id);
      if !cust_info.email.contains('@') {
        info!(
          "Sub-Handler: Invalid email '{}' for customer {}",
          cust_info.email, cust_info.customer_id
        );
        return Err(OrkaError::Internal(format!("Invalid email: {}", cust_info.email)));
      }
      cust_info.is_validated = true;
      info!(
        "Sub-Handler: Customer {} validated. Email: {}",
        cust_info.customer_id, cust_info.email
      );
      Ok(PipelineControl::Continue)
    })
  });
  pipeline.after_root("process_customer_info", |main_ctx: ContextData<OrderProcessContext>| {
    Box::pin(async move {
      let log_msg = format!("After Customer Processing: Order {}", main_ctx.read().order_id);
      info!("{}", log_msg);
      main_ctx.write().log.push(log_msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });
  pipeline.on_root("process_shipping", |ctx: ContextData<OrderProcessContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      info!("Main: Processing shipping for order {}", data.order_id);
      data.shipping_details.is_confirmed = true;
      let msg = format!(
        "Shipping confirmed for order {}: {}",
        data.order_id, data.shipping_details.address
      );
      info!("{}", msg);
      data.log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });
  pipeline.on_root("finalize_order", |ctx: ContextData<OrderProcessContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      data.is_processed = true;
      let msg = format!(
        "Order {} finalized. Customer validated (main ctx): {}, Shipping confirmed: {}",
        data.order_id, data.customer_details.is_validated, data.shipping_details.is_confirmed
      );
      info!("{}", msg);
      data.log.push(msg);
      Ok::<_, OrkaError>(PipelineControl::Continue)
    })
  });

  // --- Scenario 1: Success Path ---
  info!("\n--- Running Scenario with Valid Email (Success Path) ---");
  let initial_ctx_success = OrderProcessContext {
    order_id: "".to_string(), // Let initialize_order set it up
    ..Default::default()
  };
  let pipeline_context_success = ContextData::new(initial_ctx_success);
  let result_success = pipeline.run(pipeline_context_success.clone()).await?;
  assert_eq!(result_success, PipelineResult::Completed);
  let final_state_success = pipeline_context_success.read();
  info!("Final order state (success): {:?}", final_state_success);
  assert!(final_state_success.is_processed);
  assert!(final_state_success.shipping_details.is_confirmed);
  assert_eq!(
    final_state_success.customer_details.is_validated, false,
    "Customer validation status in main context should be unchanged (sub-handler worked on clone)."
  );

  // --- Scenario 2: Sub-handler causes an error ---
  info!("\n--- Running Scenario with Sub-Handler Error (Invalid Email) ---");
  // Create a context that will pass through initialize_order but then be "corrupted" for the sub-handler.
  // More accurately, ensure the state fed TO THE EXTRACTOR contains the invalid email.
  // `initialize_order` sets a default valid email. We need to override this for the error test.
  // The easiest way is to provide an initial context where `order_id` is already set, so `initialize_order`
  // doesn't overwrite `customer_details` with its defaults.
  let initial_ctx_error = OrderProcessContext {
    order_id: "ORD_ERR_TEST".to_string(), // Pre-set order_id
    customer_details: CustomerInfo {
      customer_id: "CUST_ERR".to_string(),
      name: "Error Test User".to_string(),
      email: "invalid-email".to_string(), // <<<< Set invalid email here
      is_validated: false,
    },
    shipping_details: ShippingInfo {
      // Need some defaults
      address: "N/A".to_string(),
      is_confirmed: false,
    },
    is_processed: false,
    log: Vec::new(),
  };
  let pipeline_ctx_error = ContextData::new(initial_ctx_error);

  let error_result = pipeline.run(pipeline_ctx_error.clone()).await;
  assert!(
    error_result.is_err(),
    "Pipeline should have failed due to sub-handler error. Result was: Ok({:?})",
    error_result.ok()
  );
  if let Err(e) = error_result {
    info!("Pipeline failed as expected: {}", e);
    assert!(format!("{:?}", e).contains("Invalid email: invalid-email"));
  }

  let final_state_error = pipeline_ctx_error.read();
  info!("Final context state (error scenario): {:?}", final_state_error);
  // `initialize_order` ran (and logged).
  // `process_customer_info` (sub-handler part) failed.
  // `after_root` for `process_customer_info` should not have run if the `on<SData>` failed.
  // `process_shipping` and `finalize_order` should not have run.
  assert!(
    final_state_error.log.len() == 1,
    "Log should only contain initialize_order message. Log: {:?}",
    final_state_error.log
  );
  assert!(final_state_error.log[0].contains("Order ORD_ERR_TEST initialized/checked"));
  assert!(!final_state_error.is_processed);
  assert!(!final_state_error.customer_details.is_validated); // Sub-handler didn't complete validation
  assert!(!final_state_error.shipping_details.is_confirmed);

  Ok(())
}
