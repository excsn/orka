// orka_project/examples/ecommerce_app/src/pipelines/checkout_pipeline.rs
use crate::errors::AppError;
use crate::pipelines::common_steps;
use crate::pipelines::contexts::{
  ActivePaymentDetails, CheckoutCtxData, MockPaymentProviderSubCtxData, SendOrderConfirmationEmailCtxData,
};
use crate::pipelines::factories::{mock_provider_a_pipeline_factory, mock_provider_b_pipeline_factory};
use crate::state::AppState;
// OrkaResult is Result<_, OrkaError>. Extractor returns OrkaResult.
use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::{error, info, instrument, warn, Level};
use uuid::Uuid;

pub fn register_checkout_pipeline(orka_registry: &Arc<orka::Orka<AppError>>, _app_state: &AppState) {
  // Pipeline is over the underlying data type CheckoutCtxData and its handlers return Result<_, AppError>
  let mut p = Pipeline::<CheckoutCtxData, AppError>::new(&[
    ("create_initial_order_record_checkout", false, None),
    ("fetch_user_details_for_checkout", false, None),
    ("determine_payment_route_and_init_sub_context_checkout", false, None),
    ("ProcessPaymentMockGateways", false, None), // This step will host conditional sub-pipelines
    ("update_order_status_post_payment_checkout", false, None),
    ("send_confirmation_email_checkout", true, None), // Optional step
  ]);

  // Step 1: Create Initial Order Record
  p.on_root(
    "create_initial_order_record_checkout",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let order_id = Uuid::new_v4();
        let user_id_for_db: Uuid;
        let cart_val_for_db: u32;
        let currency_for_db: String;
        // let db_pool_clone; // Uncomment if doing actual DB operations

        {
          let mut guard = ctx_data.write();
          guard.cart_items_value_cents = 5000;
          guard.currency_code = "USD".to_string();
          guard.order_id = Some(order_id);
          user_id_for_db = guard.authenticated_user_id;
          cart_val_for_db = guard.cart_items_value_cents;
          currency_for_db = guard.currency_code.clone();
          // db_pool_clone = guard.app_state.db_pool.clone();
        }

        info!(
          "Checkout Pipeline (Order {}): Initializing order record for user {}. Amount: {} {}",
          order_id, user_id_for_db, cart_val_for_db, currency_for_db
        );
        // Placeholder for DB op: sqlx::query!(...).execute(&db_pool_clone).await.map_err(AppError::Sqlx)?;
        info!(
          "Checkout Pipeline (Order {}): Simulated initial order record creation.",
          order_id
        );
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    },
  );

  // Step 2: Fetch User Details
  p.on_root(
    "fetch_user_details_for_checkout",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (user_id, _db_pool) = {
          // _db_pool if not used in mock
          let guard = ctx_data.read();
          (guard.authenticated_user_id, guard.app_state.db_pool.clone())
        };
        info!("Checkout Pipeline (User {}): Fetching user details.", user_id);

        // Placeholder for DB op: sqlx::query_as(...).fetch_one(&db_pool).await.map_err(AppError::from)?;
        let user_email = format!("user_{}@example.com", user_id.simple());
        let user_name = format!("User {}", user_id.simple());
        {
          let mut guard = ctx_data.write();
          guard.user_email_for_confirmation = Some(user_email);
          guard.user_name_for_confirmation = Some(user_name);
        }
        info!("Checkout Pipeline (User {}): Simulated fetching user details.", user_id);
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    },
  );

  // Step 3: Determine Payment Route and Initialize Sub-Context
  p.on_root(
    "determine_payment_route_and_init_sub_context_checkout",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (order_id_val, app_config_clone, cart_val, currency_val) = {
          let guard = ctx_data.read();
          (
            guard.order_id.expect("Order ID must be set"),
            guard.app_state.config.clone(),
            guard.cart_items_value_cents,
            guard.currency_code.clone(),
          )
        };

        let chosen_method_str;
        let account_id_for_subctx_init_str;
        if order_id_val.as_u128() % 2 == 0 {
          chosen_method_str = "mock_provider_a".to_string();
          account_id_for_subctx_init_str = app_config_clone.mock_payment_provider_main_id.clone();
        } else {
          chosen_method_str = "mock_provider_b".to_string();
          account_id_for_subctx_init_str = app_config_clone.mock_payment_provider_alt_id.clone();
        }
        info!(
          "Checkout Pipeline (Order {}): Chosen payment method: {}, Account ID for sub-ctx init: {}",
          order_id_val, chosen_method_str, account_id_for_subctx_init_str
        );

        let sub_ctx_underlying_data = MockPaymentProviderSubCtxData {
          order_id: order_id_val,
          amount_cents: cart_val,
          currency: currency_val,
          using_account_id: account_id_for_subctx_init_str.clone(), // This account ID is for the sub-pipeline
          payment_intent: None,
          succeeded: false,
        };
        let sub_ctx_data_wrapper = ContextData::new(sub_ctx_underlying_data);

        {
          let mut guard = ctx_data.write();
          guard.chosen_payment_method = chosen_method_str.clone();
          // Store the account ID that will be used by the chosen provider in the main context, if needed later
          guard.current_payment_account_id_for_sub_ctx_init = Some(account_id_for_subctx_init_str);

          match chosen_method_str.as_str() {
            "mock_provider_a" => guard.payment_sub_context = ActivePaymentDetails::MockProviderA(sub_ctx_data_wrapper),
            "mock_provider_b" => guard.payment_sub_context = ActivePaymentDetails::MockProviderB(sub_ctx_data_wrapper),
            _ => {
              error!("Unsupported payment method: {}", chosen_method_str);
              return Err(AppError::Config(format!(
                "Unsupported payment method: {}",
                chosen_method_str
              )));
            }
          }
        }
        Ok(PipelineControl::Continue)
      })
    },
  );

  // Step 4: Process Payment (Conditional Scopes)
  // The main pipeline's error type is AppError. ConditionalScopeBuilder requires Err: From<OrkaError>.
  // Our AppError already implements From<OrkaError>.
  // The factories (mock_provider_a_pipeline_factory, etc.) now return:
  // Result<Arc<Pipeline<MockPaymentProviderSubCtxData, AppError>>, OrkaError>
  // This matches the signature required by add_dynamic_scope.
  p.conditional_scopes_for_step("ProcessPaymentMockGateways")
    .add_dynamic_scope(
      // Expects factory to return Result<Arc<Pipeline<SData, AppError>>, OrkaError>
      mock_provider_a_pipeline_factory,
      // Extractor returns Result<ContextData<SData>, OrkaError>
      // This OrkaError (if extractor fails) will be converted to AppError by AnyConditionalScope impl.
      |main_ctx_data: ContextData<CheckoutCtxData>| {
        let guard = main_ctx_data.read();
        match &guard.payment_sub_context {
          ActivePaymentDetails::MockProviderA(details_sdata_wrapper) => Ok(details_sdata_wrapper.clone()),
          _ => Err(OrkaError::ExtractorFailure {
            step_name: "ProcessPaymentMockGateways_ExtractorA".to_string(),
            source: anyhow::anyhow!("Mismatched or uninitialized sub_context for MockProviderA."),
          }),
        }
      },
    )
    .on_condition(|main_ctx_data: ContextData<CheckoutCtxData>| {
      main_ctx_data.read().chosen_payment_method == "mock_provider_a"
    })
    .add_dynamic_scope(
      // Expects factory to return Result<Arc<Pipeline<SData, AppError>>, OrkaError>
      mock_provider_b_pipeline_factory,
      |main_ctx_data: ContextData<CheckoutCtxData>| {
        // Extractor
        let guard = main_ctx_data.read();
        match &guard.payment_sub_context {
          ActivePaymentDetails::MockProviderB(details_sdata_wrapper) => Ok(details_sdata_wrapper.clone()),
          _ => Err(OrkaError::ExtractorFailure {
            step_name: "ProcessPaymentMockGateways_ExtractorB".to_string(),
            source: anyhow::anyhow!("Mismatched or uninitialized sub_context for MockProviderB."),
          }),
        }
      },
    )
    .on_condition(|main_ctx_data: ContextData<CheckoutCtxData>| {
      main_ctx_data.read().chosen_payment_method == "mock_provider_b"
    })
    .if_no_scope_matches(PipelineControl::Stop)
    .finalize_conditional_step(false); // This step itself is not optional

  // After-hook for "ProcessPaymentMockGateways"
  p.after_root(
    "ProcessPaymentMockGateways",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let mut payment_was_successful = false;
        let order_id_for_log;
        {
          let guard = ctx_data.read();
          order_id_for_log = guard.order_id;
          match &guard.payment_sub_context {
            ActivePaymentDetails::MockProviderA(details) | ActivePaymentDetails::MockProviderB(details) => {
              payment_was_successful = details.read().succeeded;
            }
            ActivePaymentDetails::None => { /* payment_was_successful remains false */ }
          }
        }
        {
          ctx_data.write().payment_processing_overall_success = payment_was_successful;
        }
        info!(
          "Checkout Pipeline (Order {:?}): Payment overall success after conditional step: {}",
          order_id_for_log, payment_was_successful
        );
        if !payment_was_successful {
          return Ok::<_, AppError>(PipelineControl::Stop);
        }
        Ok(PipelineControl::Continue)
      })
    },
  );

  // Step 5: Update Order Status Post Payment
  p.on_root(
    "update_order_status_post_payment_checkout",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (order_id_val, payment_success_val, _db_pool_clone) = {
          // _db_pool_clone if not used
          let guard = ctx_data.read();
          (
            guard.order_id.expect("Order ID must be set for status update"),
            guard.payment_processing_overall_success,
            guard.app_state.db_pool.clone(),
          )
        };
        let new_status = if payment_success_val { "paid" } else { "failed" };
        info!(
          "Checkout Pipeline (Order {}): Updating order status to {}.",
          order_id_val, new_status
        );
        // Placeholder for DB op: sqlx::query!(...).execute(&db_pool_clone).await.map_err(AppError::Sqlx)?;
        {
          ctx_data.write().order_finalized_in_db = payment_success_val;
        } // Only finalized if paid

        if !payment_success_val {
          return Ok::<_, AppError>(PipelineControl::Stop);
        }
        Ok(PipelineControl::Continue)
      })
    },
  );

  // Step 6: Send Confirmation Email (Optional)
  p.on_root(
    "send_confirmation_email_checkout",
    |ctx_data: ContextData<CheckoutCtxData>| {
      Box::pin(async move { // Returns Result<PipelineControl, AppError>
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
        let order_id_val = match order_id_val_opt {
            Some(id) => id,
            None => { warn!("Skipping email: order ID missing."); return Ok::<_, AppError>(PipelineControl::Continue); }
        };
        if !should_send { info!("Skipping email for order {}: payment not successful.", order_id_val); return Ok(PipelineControl::Continue); }
        let recipient_email = match recipient_email_opt {
          Some(email) => email,
          None => { warn!("Skipping email for order {}: recipient email missing.", order_id_val); return Ok(PipelineControl::Continue); }
        };
        let recipient_name = recipient_name_opt.unwrap_or_else(|| "Valued Customer".to_string());
        let order_total_display = format!("${:.2}", order_total_val as f32 / 100.0);

        let email_ctx_data_wrapper = ContextData::new(SendOrderConfirmationEmailCtxData {
          app_state: app_state_clone, recipient_email, recipient_name, order_id: order_id_val, order_total_display,
        });

        // common_steps::send_order_confirmation_email_step returns OrkaResult<PipelineControl, OrkaError>
        // This handler must return Result<_, AppError>.
        match common_steps::send_order_confirmation_email_step(email_ctx_data_wrapper).await {
          Ok(control) => {
            info!("Order confirmation email step returned control: {:?} for order {}", control, order_id_val);
            if control == PipelineControl::Continue { ctx_data.write().confirmation_email_sent = true; }
            Ok(control) // Propagate control signal
          }
          Err(orka_err) => { // orka_err is OrkaError
            warn!("Order confirmation email step failed for order {}: {:?}", order_id_val, orka_err);
            ctx_data.write().confirmation_email_sent = false;
            // This step is optional, so we don't fail the main pipeline.
            // We log the OrkaError. If we needed to convert it to AppError to stop the main pipeline,
            // we would do: return Err(AppError::from(orka_err));
            Ok(PipelineControl::Continue)
          }
        }
      })
    },
  );

  orka_registry.register_pipeline(p);
  info!("Checkout pipeline registered.");
}
