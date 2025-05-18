// orka_project/examples/ecommerce_app/src/pipelines/common_steps.rs
use crate::errors::{AppError, Result as AppResult};
use crate::pipelines::contexts::{SendOrderConfirmationEmailCtxData, SendWelcomeEmailCtxData}; // Use *CtxData
use crate::services::{auth_service, email_mock};
use crate::state::AppState;
use orka::{ContextData, OrkaError, OrkaResult, PipelineControl}; // Added ContextData, OrkaError
use tracing::{info, instrument, warn, Level};
use uuid::Uuid;

// ... (verify_password_step would also change if it was part of a Ctx, but it takes primitives directly)

#[instrument(name = "common_step::send_welcome_email", skip(ctx_data), err)]
pub async fn send_welcome_email_step(ctx_data: ContextData<SendWelcomeEmailCtxData>) -> OrkaResult<PipelineControl> {
  // Read needed data from ctx_data
  let (recipient_email_clone, recipient_name_clone, app_config_clone) = {
    let guard = ctx_data.read();
    info!("Attempting to send welcome email to {}", guard.recipient_email);
    (guard.recipient_email.clone(), guard.recipient_name.clone(), guard.app_state.config.clone())
  }; // guard dropped

  match email_mock::send_mock_email(
    &recipient_email_clone,
    &app_config_clone.mock_email_sender,
    &format!("Welcome to OrkaCommerce, {}!", recipient_name_clone),
    &format!(
      "<p>Hi {},</p><p>Thanks for signing up to OrkaCommerce!</p>",
      recipient_name_clone
    ),
  ).await { // .await after lock is dropped
    Ok(sent_info) => {
      info!(
        "Welcome email sent successfully to {}. Message ID: {}",
        recipient_email_clone, sent_info.message_id
      );
      // If SendWelcomeEmailCtxData had a field for message_id, write it back:
      // ctx_data.write().email_message_id = Some(sent_info.message_id);
      Ok(PipelineControl::Continue)
    }
    Err(e) => { // e is AppError
      warn!("Failed to send welcome email to {}: {:?}", recipient_email_clone, e);
      Err(OrkaError::HandlerError { source: anyhow::Error::new(e) }) // Wrap AppError
    }
  }
}

#[instrument(name = "common_step::send_order_confirmation", skip(ctx_data), err)]
pub async fn send_order_confirmation_email_step(
  ctx_data: ContextData<SendOrderConfirmationEmailCtxData>,
) -> OrkaResult<PipelineControl> {
  let (recipient_email_clone, recipient_name_clone, order_id_val, order_total_display_val, app_config_clone) = {
    let guard = ctx_data.read();
    info!(
      "Attempting to send order confirmation for order {} to {}",
      guard.order_id, guard.recipient_email
    );
    (
      guard.recipient_email.clone(), 
      guard.recipient_name.clone(), 
      guard.order_id, 
      guard.order_total_display.clone(), 
      guard.app_state.config.clone()
    )
  }; // guard dropped

  match email_mock::send_mock_email(
    &recipient_email_clone,
    &app_config_clone.mock_email_sender,
    &format!("Your OrkaCommerce Order #{} is Confirmed!", order_id_val),
    &format!(
      "<p>Hi {},</p><p>Your order #{} for {} has been confirmed.</p><p>Thank you for your purchase!</p>",
      recipient_name_clone, order_id_val, order_total_display_val
    ),
  ).await { // .await after lock is dropped
    Ok(sent_info) => {
      info!(
        "Order confirmation email sent successfully to {}. Message ID: {}",
        recipient_email_clone, sent_info.message_id
      );
      Ok(PipelineControl::Continue)
    }
    Err(e) => { // e is AppError
      warn!(
        "Failed to send order confirmation email for order {} to {}: {:?}",
        order_id_val, recipient_email_clone, e
      );
      Err(OrkaError::HandlerError { source: anyhow::Error::new(e) }) // Wrap AppError
    }
  }
}

// ... (check_user_exists_step - if it were to take ContextData, it would be similar)
// If check_user_exists_step is more of a direct service call, it might not need ContextData,
// but if it's a "pipeline step", it implies it operates on a pipeline's context.
// For verify_password_step, since it takes primitives, it's fine as is. The *caller*
// (an Orka handler) would extract those primitives from its ContextData.