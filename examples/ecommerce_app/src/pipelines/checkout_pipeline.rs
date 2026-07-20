use crate::errors::AppError;
use crate::pipelines::common_steps;
use crate::pipelines::contexts::{CheckoutCtxData, MockPaymentProviderSubCtxData, SendOrderConfirmationEmailCtxData};
use crate::pipelines::factories::{mock_provider_a_pipeline_factory, mock_provider_b_pipeline_factory};
use crate::state::AppState;
use orka::{ContextData, OrkaError, OrkaResult, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

/// Builds the payment scope's own context from the checkout context. The scoped pipeline
/// works on this detached copy and it is folded back via `with_merge`.
fn extract_payment_sub_context(
  main_ctx_data: ContextData<CheckoutCtxData>,
  step_name: &str,
) -> Result<ContextData<MockPaymentProviderSubCtxData>, OrkaError> {
  let guard = main_ctx_data.read();

  let fail = |message: &str| OrkaError::ExtractorFailure {
    step_name: step_name.to_string(),
    source: anyhow::anyhow!("{}", message),
  };

  let order_id = guard.order_id.ok_or_else(|| fail("Order ID not set before payment processing."))?;
  let using_account_id = guard
    .current_payment_account_id_for_sub_ctx_init
    .clone()
    .ok_or_else(|| fail("Payment account ID not set before payment processing."))?;

  Ok(ContextData::new(MockPaymentProviderSubCtxData {
    order_id,
    amount_cents: guard.cart_items_value_cents,
    currency: guard.currency_code.clone(),
    using_account_id,
    payment_intent: None,
    succeeded: false,
  }))
}

pub fn register_checkout_pipeline(orka_registry: &Arc<orka::Orka<AppError>>, _app_state: &AppState) -> OrkaResult<()> {
  let mut p = Pipeline::<CheckoutCtxData, AppError>::new([
    "create_initial_order_record_checkout",
    "fetch_user_details_for_checkout",
    "determine_payment_route_checkout",
    "ProcessPaymentMockGateways",
    "update_order_status_post_payment_checkout",
    "send_confirmation_email_checkout",
  ]);
  p.optional("send_confirmation_email_checkout");

  p.on_root("create_initial_order_record_checkout", |ctx_data| async move {
    let order_id = Uuid::new_v4();
    let (user_id_for_db, cart_val_for_db, currency_for_db) = {
      let mut guard = ctx_data.write();
      guard.cart_items_value_cents = 5000;
      guard.currency_code = "USD".to_string();
      guard.order_id = Some(order_id);
      (
        guard.authenticated_user_id,
        guard.cart_items_value_cents,
        guard.currency_code.clone(),
      )
    };

    info!(
      "Checkout Pipeline (Order {}): Initializing order record for user {}. Amount: {} {}",
      order_id, user_id_for_db, cart_val_for_db, currency_for_db
    );
    info!(
      "Checkout Pipeline (Order {}): Simulated initial order record creation.",
      order_id
    );
    Ok(PipelineControl::Continue)
  })
  .on_root("fetch_user_details_for_checkout", |ctx_data| async move {
    let user_id = { ctx_data.read().authenticated_user_id };
    info!("Checkout Pipeline (User {}): Fetching user details.", user_id);

    let user_email = format!("user_{}@example.com", user_id.simple());
    let user_name = format!("User {}", user_id.simple());
    {
      let mut guard = ctx_data.write();
      guard.user_email_for_confirmation = Some(user_email);
      guard.user_name_for_confirmation = Some(user_name);
    }
    info!("Checkout Pipeline (User {}): Simulated fetching user details.", user_id);
    Ok(PipelineControl::Continue)
  })
  .on_root("determine_payment_route_checkout", |ctx_data| async move {
    let (order_id_val, app_config_clone) = {
      let guard = ctx_data.read();
      (guard.order_id.expect("Order ID must be set"), guard.app_state.config.clone())
    };

    let (chosen_method_str, account_id_str) = if order_id_val.as_u128() % 2 == 0 {
      (
        "mock_provider_a".to_string(),
        app_config_clone.mock_payment_provider_main_id.clone(),
      )
    } else {
      (
        "mock_provider_b".to_string(),
        app_config_clone.mock_payment_provider_alt_id.clone(),
      )
    };
    info!(
      "Checkout Pipeline (Order {}): Chosen payment method: {}, Account ID: {}",
      order_id_val, chosen_method_str, account_id_str
    );

    {
      let mut guard = ctx_data.write();
      guard.chosen_payment_method = chosen_method_str;
      guard.current_payment_account_id_for_sub_ctx_init = Some(account_id_str);
    }
    Ok(PipelineControl::Continue)
  });

  p.conditional_scopes_for_step("ProcessPaymentMockGateways")
    .add_dynamic_scope(mock_provider_a_pipeline_factory, |main_ctx_data| {
      extract_payment_sub_context(main_ctx_data, "ProcessPaymentMockGateways_ExtractorA")
    })
    .with_merge(|main, sub| main.payment_result = Some(sub.clone()))
    .on_condition(|main_ctx_data: ContextData<CheckoutCtxData>| {
      main_ctx_data.read().chosen_payment_method == "mock_provider_a"
    })
    .add_dynamic_scope(mock_provider_b_pipeline_factory, |main_ctx_data| {
      extract_payment_sub_context(main_ctx_data, "ProcessPaymentMockGateways_ExtractorB")
    })
    .with_merge(|main, sub| main.payment_result = Some(sub.clone()))
    .on_condition(|main_ctx_data: ContextData<CheckoutCtxData>| {
      main_ctx_data.read().chosen_payment_method == "mock_provider_b"
    })
    .if_no_scope_matches(PipelineControl::Stop)
    .finalize_conditional_step(false);

  p.after_root("ProcessPaymentMockGateways", |ctx_data| async move {
    let (order_id_for_log, payment_was_successful) = {
      let mut guard = ctx_data.write();
      let succeeded = guard.payment_result.as_ref().is_some_and(|r| r.succeeded);
      guard.payment_processing_overall_success = succeeded;
      (guard.order_id, succeeded)
    };
    info!(
      "Checkout Pipeline (Order {:?}): Payment overall success after conditional step: {}",
      order_id_for_log, payment_was_successful
    );
    if !payment_was_successful {
      return Ok(PipelineControl::Stop);
    }
    Ok(PipelineControl::Continue)
  })
  .on_root("update_order_status_post_payment_checkout", |ctx_data| async move {
    let (order_id_val, payment_success_val) = {
      let guard = ctx_data.read();
      (
        guard.order_id.expect("Order ID must be set for status update"),
        guard.payment_processing_overall_success,
      )
    };
    let new_status = if payment_success_val { "paid" } else { "failed" };
    info!(
      "Checkout Pipeline (Order {}): Updating order status to {}.",
      order_id_val, new_status
    );
    ctx_data.write().order_finalized_in_db = payment_success_val;

    if !payment_success_val {
      return Ok(PipelineControl::Stop);
    }
    Ok(PipelineControl::Continue)
  })
  .on_root("send_confirmation_email_checkout", |ctx_data| async move {
    let (should_send, app_state_clone, recipient_email_opt, recipient_name_opt, order_id_val_opt, order_total_val) = {
      let guard = ctx_data.read();
      (
        guard.payment_processing_overall_success,
        guard.app_state.clone(),
        guard.user_email_for_confirmation.clone(),
        guard.user_name_for_confirmation.clone(),
        guard.order_id,
        guard.cart_items_value_cents,
      )
    };

    let Some(order_id_val) = order_id_val_opt else {
      warn!("Skipping email: order ID missing.");
      return Ok(PipelineControl::Continue);
    };
    if !should_send {
      info!("Skipping email for order {}: payment not successful.", order_id_val);
      return Ok(PipelineControl::Continue);
    }
    let Some(recipient_email) = recipient_email_opt else {
      warn!("Skipping email for order {}: recipient email missing.", order_id_val);
      return Ok(PipelineControl::Continue);
    };
    let recipient_name = recipient_name_opt.unwrap_or_else(|| "Valued Customer".to_string());
    let order_total_display = format!("${:.2}", order_total_val as f32 / 100.0);

    let email_ctx_data_wrapper = ContextData::new(SendOrderConfirmationEmailCtxData {
      app_state: app_state_clone,
      recipient_email,
      recipient_name,
      order_id: order_id_val,
      order_total_display,
    });

    match common_steps::send_order_confirmation_email_step(email_ctx_data_wrapper).await {
      Ok(control) => {
        info!(
          "Order confirmation email step returned control: {:?} for order {}",
          control, order_id_val
        );
        if control == PipelineControl::Continue {
          ctx_data.write().confirmation_email_sent = true;
        }
        Ok(control)
      }
      Err(orka_err) => {
        warn!(
          "Order confirmation email step failed for order {}: {:?}",
          order_id_val, orka_err
        );
        ctx_data.write().confirmation_email_sent = false;
        // This step is optional: a failed email must not fail the checkout.
        Ok(PipelineControl::Continue)
      }
    }
  });

  orka_registry.register_pipeline(p)?;
  info!("Checkout pipeline registered.");
  Ok(())
}
