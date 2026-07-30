use actix_web::{web, HttpResponse};
use serde_json::json;
use tracing::{info, instrument, warn};

use crate::errors::AppError;
use crate::pipelines::contexts::CheckoutCtxData;
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

use super::cart_handlers::AuthenticatedUser;

#[instrument(
    name = "handler::start_checkout",
    skip(app_state, auth_user),
    fields(user_id = %auth_user.user_id)
)]
pub async fn start_checkout_handler(
  app_state: web::Data<AppState>,
  auth_user: AuthenticatedUser,
) -> Result<HttpResponse, AppError> {
  info!("Checkout initiation attempt by user: {}", auth_user.user_id);
  let checkout_ctx_initial = CheckoutCtxData {
    app_state: app_state.get_ref().clone(),
    authenticated_user_id: auth_user.user_id,
    // Everything below is filled in by the checkout pipeline.
    order_id: None,
    cart_items_value_cents: 0,
    currency_code: String::new(),
    chosen_payment_method: String::new(),
    current_payment_account_id_for_sub_ctx_init: None,
    payment_result: None,
    payment_processing_overall_success: false,
    order_finalized_in_db: false,
    confirmation_email_sent: false,
    user_email_for_confirmation: None,
    user_name_for_confirmation: None,
  };
  let orka_context_data = ContextData::new(checkout_ctx_initial);
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
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
      Ok(HttpResponse::Ok().json(json!({
                "message": if payment_successful { "Checkout successful." } else { "Checkout processed, but payment was not successful." },
                "orderId": order_id.to_string(),
                "paymentSuccess": payment_successful,
                "confirmationEmailSent": email_sent,
                // A frontend handling SCA would also want the intent's client_secret,
                // available via final_ctx_guard.payment_result.
            })))
    }
    Ok(PipelineResult::Stopped) => {
      // A declined payment stops the pipeline rather than erroring; that is an expected outcome.
      let final_ctx_guard = orka_context_data.read();
      warn!(
        "Checkout pipeline for user {} was stopped by a handler. Payment success: {}. Order ID: {:?}",
        auth_user.user_id, final_ctx_guard.payment_processing_overall_success, final_ctx_guard.order_id
      );

      if !final_ctx_guard.payment_processing_overall_success {
        Err(AppError::Payment(
          "Payment processing failed or was cancelled.".to_string(),
        ))
      } else {
        Err(AppError::Internal(
          "Checkout process was halted after payment.".to_string(),
        ))
      }
    }
    Ok(other) => {
      warn!(
        "Checkout pipeline for user {} ended as {:?}.",
        auth_user.user_id, other
      );
      Err(AppError::Internal(
        "Checkout process did not run to completion.".to_string(),
      ))
    }
    Err(app_err) => {
      warn!("Checkout pipeline failed for user {}: {:?}", auth_user.user_id, app_err);
      Err(app_err)
    }
  }
}
