// examples/ecommerce_app/src/web/handlers/checkout_handlers.rs

use actix_web::{web, HttpResponse, Responder};
// No specific request payload for simple checkout initiation if cart is read from DB by pipeline.
// If payload is needed (e.g., shipping address, payment method ID), define a DTO.
// use serde::Deserialize;
use serde_json::json;
use tracing::{info, instrument, warn, Level};

use crate::errors::AppError;
use crate::pipelines::contexts::CheckoutCtxData; // Correct context
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

// Re-using the placeholder AuthenticatedUser extractor from cart_handlers
// In a real app, this would be in a shared location (e.g., web/extractors.rs)
use super::cart_handlers::AuthenticatedUser; // Assuming cart_handlers.rs is in the same `handlers` module

// --- Handler Implementation ---

#[instrument(
    name = "handler::start_checkout",
    skip(app_state, auth_user),
    fields(user_id = %auth_user.user_id)
)]
pub async fn start_checkout_handler(
  app_state: web::Data<AppState>,
  auth_user: AuthenticatedUser, // Extracted authenticated user
                                // req_payload: Option<web::Json<CheckoutRequestPayload>>, // If you have a request body
) -> Result<HttpResponse, AppError> {
  info!("Checkout initiation attempt by user: {}", auth_user.user_id);

  // 1. Prepare the initial context data for the checkout pipeline
  let checkout_ctx_initial = CheckoutCtxData {
    app_state: app_state.get_ref().clone(),
    authenticated_user_id: auth_user.user_id,
    // These will be populated by the pipeline or from DB lookups within the pipeline:
    order_id: None,
    cart_items_value_cents: 0,            // Will be calculated/fetched by the pipeline
    currency_code: String::new(),         // Will be set by the pipeline
    chosen_payment_method: String::new(), // Will be determined by the pipeline
    current_payment_account_id_for_sub_ctx_init: None, // Will be set by the pipeline
    payment_sub_context: crate::pipelines::contexts::ActivePaymentDetails::None,
    payment_processing_overall_success: false,
    order_finalized_in_db: false,
    confirmation_email_sent: false,
    user_email_for_confirmation: None,
    user_name_for_confirmation: None,
  };
  let orka_context_data = ContextData::new(checkout_ctx_initial);

  // 2. Run the checkout pipeline
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
      // Pipeline completed successfully (payment processed, order updated, email sent - or marked as optional if failed).
      let final_ctx_guard = orka_context_data.read();
      let order_id = final_ctx_guard.order_id.ok_or_else(|| {
        warn!(
          "Checkout pipeline completed for user {} but order_id was not set.",
          auth_user.user_id
        );
        AppError::Internal("Checkout process completed, but order confirmation details are unavailable.".to_string())
      })?;
      let payment_successful = final_ctx_guard.payment_processing_overall_success;
      let email_sent = final_ctx_guard.confirmation_email_sent;

      info!(
        "Checkout process completed for user: {}. Order ID: {}. Payment success: {}. Email sent: {}",
        auth_user.user_id, order_id, payment_successful, email_sent
      );

      // 3. Construct and return HTTP response
      // The response depends on what information is most useful to the client after checkout.
      // Typically, the order ID and final status are important.
      Ok(HttpResponse::Ok().json(json!({
                "message": if payment_successful { "Checkout successful." } else { "Checkout processed, but payment was not successful." },
                "orderId": order_id.to_string(),
                "paymentSuccess": payment_successful,
                "confirmationEmailSent": email_sent,
                // You might include payment_intent client_secret if frontend needs to handle SCA
                // This would need to be extracted from final_ctx_guard.payment_sub_context
            })))
    }
    Ok(PipelineResult::Stopped) => {
      // Pipeline was explicitly stopped. This could be due to payment failure handled within
      // the pipeline (which is a valid outcome), or other business rule.
      let final_ctx_guard = orka_context_data.read();
      warn!(
        "Checkout pipeline for user {} was stopped by a handler. Payment success: {}. Order ID: {:?}",
        auth_user.user_id, final_ctx_guard.payment_processing_overall_success, final_ctx_guard.order_id
      );

      // If it stopped due to payment failure, this is an expected "failure" path from client perspective.
      if !final_ctx_guard.payment_processing_overall_success {
        Err(AppError::Payment(
          "Payment processing failed or was cancelled.".to_string(),
        ))
      } else {
        // Stopped for another reason after successful payment (unlikely in this flow but possible)
        Err(AppError::Internal(
          "Checkout process was halted after payment.".to_string(),
        ))
      }
    }
    Err(app_err) => {
      // Handles AppError from within the pipeline (e.g., DB error, validation, config error).
      warn!("Checkout pipeline failed for user {}: {:?}", auth_user.user_id, app_err);
      Err(app_err)
    }
  }
}
