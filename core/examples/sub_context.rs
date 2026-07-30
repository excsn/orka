use orka::{ContextData, OrkaError, Pipeline, PipelineControl, PipelineResult};
use tracing::info;

// --- Contexts ---
#[derive(Clone, Debug, Default)]
struct OrderProcessContext {
  order_id: String,
  customer_details: CustomerInfo,
  shipping_details: ShippingInfo,
  is_processed: bool,
  log: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct CustomerInfo {
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

/// Builds the order pipeline. `merge_customer` picks which flavour of extractor the
/// `process_customer_info` step uses: a plain detached one, or one that folds the
/// sub-context back into the root context.
fn build_order_pipeline(merge_customer: bool) -> Pipeline<OrderProcessContext, OrkaError> {
  let mut pipeline = Pipeline::<OrderProcessContext, OrkaError>::new([
    "initialize_order",
    "process_customer_info",
    "process_shipping",
    "finalize_order",
  ]);

  pipeline.on_root("initialize_order", |ctx| async move {
    let mut data = ctx.write();
    // Leave a caller-supplied order alone, so scenarios can seed their own state.
    if data.order_id.is_empty() {
      data.order_id = "ORD123".to_string();
      data.customer_details = CustomerInfo {
        customer_id: "CUST456".to_string(),
        name: "John Doe".to_string(),
        email: "john.doe@example.com".to_string(),
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
    Ok(PipelineControl::Continue)
  });

  if merge_customer {
    pipeline.set_extractor_with_merge(
      "process_customer_info",
      |main_ctx: ContextData<OrderProcessContext>| {
        info!(
          "Extractor (with merge): Extracting CustomerInfo for order {}",
          main_ctx.read().order_id
        );
        Ok(main_ctx.project(|d| d.customer_details.clone()))
      },
      |root, sub| root.customer_details = sub.clone(),
    );
  } else {
    pipeline.set_extractor("process_customer_info", |main_ctx: ContextData<OrderProcessContext>| {
      info!(
        "Extractor (detached): Extracting CustomerInfo for order {}",
        main_ctx.read().order_id
      );
      Ok(main_ctx.project(|d| d.customer_details.clone()))
    });
  }

  pipeline
    .on("process_customer_info", |s_ctx: ContextData<CustomerInfo>| async move {
      let mut cust_info = s_ctx.write();
      info!(
        "Sub-Handler: Processing customer {} ({})",
        cust_info.customer_id, cust_info.name
      );
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
    .after_root("process_customer_info", |main_ctx| async move {
      let log_msg = format!("After Customer Processing: Order {}", main_ctx.read().order_id);
      info!("{}", log_msg);
      main_ctx.write().log.push(log_msg);
      Ok(PipelineControl::Continue)
    })
    .on_root("process_shipping", |ctx| async move {
      let mut data = ctx.write();
      info!("Main: Processing shipping for order {}", data.order_id);
      data.shipping_details.is_confirmed = true;
      let msg = format!(
        "Shipping confirmed for order {}: {}",
        data.order_id, data.shipping_details.address
      );
      info!("{}", msg);
      data.log.push(msg);
      Ok(PipelineControl::Continue)
    })
    .on_root("finalize_order", |ctx| async move {
      let mut data = ctx.write();
      data.is_processed = true;
      let msg = format!(
        "Order {} finalized. Customer validated (main ctx): {}, Shipping confirmed: {}",
        data.order_id, data.customer_details.is_validated, data.shipping_details.is_confirmed
      );
      info!("{}", msg);
      data.log.push(msg);
      Ok(PipelineControl::Continue)
    });

  pipeline
}

#[tokio::main]
async fn main() -> Result<(), OrkaError> {
  tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
  info!("--- Sub-Context Extraction Example (Non-Conditional) ---");

  let detached_pipeline = build_order_pipeline(false);

  // --- Scenario 1: `set_extractor` gives the sub-handler a detached copy ---
  // The sub-handler sets `is_validated`, but it does so on its own `ContextData`, so the
  // root context never sees it.
  info!("\n--- Scenario 1: detached extractor (writes are discarded) ---");
  let ctx_detached = ContextData::new(OrderProcessContext::default());
  let result_detached = detached_pipeline.run(ctx_detached.clone()).await?;
  assert_eq!(result_detached, PipelineResult::Completed);
  {
    // Scope the guard: it must not still be held when the next scenario awaits.
    let final_detached = ctx_detached.read();
    info!("Final order state (detached): {:?}", final_detached);
    assert!(final_detached.is_processed);
    assert!(final_detached.shipping_details.is_confirmed);
    assert!(
      !final_detached.customer_details.is_validated,
      "Detached extractor: the sub-handler worked on a clone, so the main context is unchanged."
    );
  }

  // --- Scenario 2: `set_extractor_with_merge` folds the sub-context back in ---
  // Same handlers, same sub-context; the only difference is the merge function, which
  // runs after the sub-handler succeeds and copies its work into the root context.
  info!("\n--- Scenario 2: extractor with merge (writes land in the parent) ---");
  let merging_pipeline = build_order_pipeline(true);
  let ctx_merged = ContextData::new(OrderProcessContext::default());
  let result_merged = merging_pipeline.run(ctx_merged.clone()).await?;
  assert_eq!(result_merged, PipelineResult::Completed);
  {
    let final_merged = ctx_merged.read();
    info!("Final order state (merged): {:?}", final_merged);
    assert!(final_merged.is_processed);
    assert!(
      final_merged.customer_details.is_validated,
      "Merging extractor: the sub-handler's validation should be visible in the main context."
    );
  }

  // --- Scenario 3: the sub-handler fails ---
  // Seeding `order_id` stops `initialize_order` from overwriting the invalid email.
  info!("\n--- Scenario 3: sub-handler error (invalid email) ---");
  let ctx_error = ContextData::new(OrderProcessContext {
    order_id: "ORD_ERR_TEST".to_string(),
    customer_details: CustomerInfo {
      customer_id: "CUST_ERR".to_string(),
      name: "Error Test User".to_string(),
      email: "invalid-email".to_string(),
      is_validated: false,
    },
    shipping_details: ShippingInfo {
      address: "N/A".to_string(),
      is_confirmed: false,
    },
    is_processed: false,
    log: Vec::new(),
  });

  let error_result = merging_pipeline.run(ctx_error.clone()).await;
  assert!(
    error_result.is_err(),
    "Pipeline should have failed due to sub-handler error. Result was: Ok({:?})",
    error_result.ok()
  );
  if let Err(e) = error_result {
    info!("Pipeline failed as expected: {}", e);
    assert!(format!("{:?}", e).contains("Invalid email: invalid-email"));
  }

  let final_error = ctx_error.read();
  info!("Final context state (error scenario): {:?}", final_error);
  // Only `initialize_order` ran: the failing sub-handler aborts before `after_root` and
  // before the remaining steps, and a failed sub-handler is never merged back.
  assert_eq!(
    final_error.log.len(),
    1,
    "Log should only contain initialize_order message. Log: {:?}",
    final_error.log
  );
  assert!(final_error.log[0].contains("Order ORD_ERR_TEST initialized/checked"));
  assert!(!final_error.is_processed);
  assert!(!final_error.customer_details.is_validated);
  assert!(!final_error.shipping_details.is_confirmed);

  Ok(())
}
