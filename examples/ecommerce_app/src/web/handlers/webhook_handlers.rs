// examples/ecommerce_app/src/web/handlers/webhook_handlers.rs

use actix_web::{web, HttpRequest, HttpResponse, Responder};
use tracing::{error, info, instrument, warn, Level};

use crate::errors::AppError;
use crate::pipelines::contexts::GenericWebhookCtxData;
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

// --- Handler Implementation ---

#[instrument(
    name = "handler::generic_webhook",
    skip(app_state, req, body),
    fields(webhook_source = %webhook_source, content_type = ?req.headers().get("content-type").map(|h| h.to_str().unwrap_or_default()))
)]
pub async fn generic_webhook_handler(
  app_state: web::Data<AppState>,
  req: HttpRequest,                  // To access headers and path parameters
  webhook_source: web::Path<String>, // Extract the {source} from the path
  body: web::Bytes,                  // Raw request body
) -> Result<HttpResponse, AppError> {
  let source_identifier = webhook_source.into_inner();
  info!(
    "Received webhook for source: '{}'. Payload size: {} bytes.",
    source_identifier,
    body.len()
  );

  // 1. Extract relevant headers (e.g., signature)
  // Example: Stripe sends signature in "Stripe-Signature" header
  let signature_header = req
    .headers()
    .get("stripe-signature") // Adjust header name based on actual provider
    .and_then(|h_val| h_val.to_str().ok())
    .map(String::from);

  if signature_header.is_some() {
    info!(
      "Webhook for source '{}' contained a signature header.",
      source_identifier
    );
  }

  // 2. Prepare the initial context data for the generic webhook pipeline
  let webhook_ctx_initial = GenericWebhookCtxData {
    app_state: app_state.get_ref().clone(),
    raw_payload: body, // web::Bytes is cloneable (Arc internally)
    source_identifier: source_identifier.clone(),
    signature_header,
    // These will be updated by the pipeline:
    event_processed: false,
    affected_order_id: None,
  };
  let orka_context_data = ContextData::new(webhook_ctx_initial);

  // 3. Run the generic webhook pipeline
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
      // Pipeline completed. This typically means the webhook was validated, parsed,
      // and routed for processing (even if that processing is async in a sub-pipeline).
      // The key is to acknowledge receipt to the webhook provider quickly.
      let final_ctx_guard = orka_context_data.read();
      info!(
        "Webhook pipeline completed for source: '{}'. Event processed flag: {}. Affected order: {:?}",
        source_identifier, final_ctx_guard.event_processed, final_ctx_guard.affected_order_id
      );

      // Return 200 OK to acknowledge receipt.
      // Specifics of what to return in the body (if anything) depend on the webhook provider's expectations.
      // Often, an empty 200 OK is sufficient.
      Ok(HttpResponse::Ok().finish()) // .json({"status": "received"})
    }
    Ok(PipelineResult::Stopped) => {
      // Pipeline was stopped. This could be due to an unhandled event type,
      // signature verification failure if not an error, etc.
      // Usually, for webhooks, even a "stopped" pipeline (if it means "event not applicable")
      // should result in a 200 OK to prevent retries, unless it's a genuine processing error
      // that the sender should be aware of.
      warn!(
        "Webhook pipeline for source '{}' was stopped. This might be an unhandled event or an issue.",
        source_identifier
      );
      // Depending on why it stopped, you might still return 200 OK or a specific error.
      // If signature verification failed and returned PipelineControl::Stop, it should have been an AppError.
      // For now, let's assume a stop means "not processed further but acknowledged."
      Ok(HttpResponse::Ok().json(serde_json::json!({"status": "acknowledged_stopped"})))
    }
    Err(app_err) => {
      // An error occurred during pipeline execution (e.g., signature verification failure that returned Err,
      // payload parsing failure, error during sub-pipeline dispatch or execution).
      error!(
        "Webhook pipeline for source '{}' failed: {:?}",
        source_identifier, app_err
      );
      // Returning the AppError will trigger its ResponseError impl,
      // sending an appropriate HTTP error code (e.g., 400 for validation/auth, 500 for internal).
      // This might cause the webhook provider to retry.
      Err(app_err)
    }
  }
}
