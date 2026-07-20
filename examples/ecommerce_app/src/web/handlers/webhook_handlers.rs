use actix_web::{web, HttpRequest, HttpResponse};
use tracing::{error, info, instrument, warn};

use crate::errors::AppError;
use crate::pipelines::contexts::GenericWebhookCtxData;
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

#[instrument(
    name = "handler::generic_webhook",
    skip(app_state, req, body),
    fields(webhook_source = %webhook_source, content_type = ?req.headers().get("content-type").map(|h| h.to_str().unwrap_or_default()))
)]
pub async fn generic_webhook_handler(
  app_state: web::Data<AppState>,
  req: HttpRequest,
  webhook_source: web::Path<String>,
  body: web::Bytes,
) -> Result<HttpResponse, AppError> {
  let source_identifier = webhook_source.into_inner();
  info!(
    "Received webhook for source: '{}'. Payload size: {} bytes.",
    source_identifier,
    body.len()
  );
  let signature_header = req
    .headers()
    .get("stripe-signature")
    .and_then(|h_val| h_val.to_str().ok())
    .map(String::from);

  if signature_header.is_some() {
    info!(
      "Webhook for source '{}' contained a signature header.",
      source_identifier
    );
  }
  let webhook_ctx_initial = GenericWebhookCtxData {
    app_state: app_state.get_ref().clone(),
    raw_payload: body,
    source_identifier: source_identifier.clone(),
    signature_header,
    event_processed: false,
    affected_order_id: None,
  };
  let orka_context_data = ContextData::new(webhook_ctx_initial);

  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
      let final_ctx_guard = orka_context_data.read();
      info!(
        "Webhook pipeline completed for source: '{}'. Event processed flag: {}. Affected order: {:?}",
        source_identifier, final_ctx_guard.event_processed, final_ctx_guard.affected_order_id
      );

      Ok(HttpResponse::Ok().finish())
    }
    Ok(PipelineResult::Stopped) => {
      // A stop means "event not applicable"; still answer 200 so the sender does not retry.
      warn!(
        "Webhook pipeline for source '{}' was stopped. This might be an unhandled event or an issue.",
        source_identifier
      );
      Ok(HttpResponse::Ok().json(serde_json::json!({"status": "acknowledged_stopped"})))
    }
    Err(app_err) => {
      error!(
        "Webhook pipeline for source '{}' failed: {:?}",
        source_identifier, app_err
      );
      // ResponseError turns this into a 4xx/5xx, which may make the provider retry.
      Err(app_err)
    }
  }
}
