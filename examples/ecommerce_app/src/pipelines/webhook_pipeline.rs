use crate::errors::AppError;
use crate::pipelines::contexts::GenericWebhookCtxData;
use crate::state::AppState;
use orka::{ContextData, Orka, OrkaError, OrkaResult, Pipeline, PipelineControl};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tracing::{error, info, warn};

/// Sub-pipeline for a specific event type. A real app would have one factory per
/// event type / source; this one just logs the decoded event.
async fn mock_payment_event_pipeline_factory(
  _main_webhook_ctx: ContextData<GenericWebhookCtxData>,
) -> Result<Arc<Pipeline<JsonValue, AppError>>, OrkaError> {
  info!("Factory: Creating mock payment event processing pipeline.");
  let mut p = Pipeline::<JsonValue, AppError>::new(["process_payment_success_event_detail"]);

  p.on_root("process_payment_success_event_detail", |event_data_ctx| async move {
    let event_data = event_data_ctx.read();
    info!("Mock Payment Event Sub-Pipeline: Processing event: {:?}", *event_data);
    Ok(PipelineControl::Continue)
  });
  Ok(Arc::new(p))
}

pub fn register_webhook_pipeline(orka_registry: &Arc<Orka<AppError>>, _app_state: &AppState) -> OrkaResult<()> {
  let mut p = Pipeline::<GenericWebhookCtxData, AppError>::new([
    "verify_webhook_signature",
    "parse_webhook_payload",
    "route_webhook_event",
    "acknowledge_webhook_receipt",
  ]);
  p.optional("verify_webhook_signature");

  p.on_root("verify_webhook_signature", |ctx_data| async move {
    let (source_id, signature_opt) = {
      let guard = ctx_data.read();
      (guard.source_identifier.clone(), guard.signature_header.clone())
    };

    info!(
      "Webhook Pipeline: Verifying signature for source '{}'. Signature provided: {}",
      source_id,
      signature_opt.is_some()
    );

    // A real implementation would HMAC the raw payload with the provider's secret.
    if source_id == "critical_source_requires_signature" && signature_opt.is_none() {
      warn!(
        "Webhook Pipeline: Signature missing for critical source '{}'.",
        source_id
      );
      return Err(AppError::Auth(
        "Webhook signature verification failed: Missing signature.".to_string(),
      ));
    }
    if let Some(signature) = signature_opt {
      if signature == "invalid_test_signature" {
        warn!(
          "Webhook Pipeline: Invalid signature received for source '{}'.",
          source_id
        );
        return Err(AppError::Auth(
          "Webhook signature verification failed: Invalid signature.".to_string(),
        ));
      }
      info!(
        "Webhook Pipeline: Signature for source '{}' deemed valid (mock).",
        source_id
      );
    } else {
      info!(
        "Webhook Pipeline: No signature provided or not required for source '{}'. Skipping verification.",
        source_id
      );
    }
    Ok(PipelineControl::Continue)
  })
  .on_root("parse_webhook_payload", |ctx_data| async move {
    let (raw_payload_bytes, source_id) = {
      let guard = ctx_data.read();
      (guard.raw_payload.clone(), guard.source_identifier.clone())
    };

    info!(
      "Webhook Pipeline: Parsing payload for source '{}'. Payload size: {} bytes.",
      source_id,
      raw_payload_bytes.len()
    );

    match serde_json::from_slice::<JsonValue>(&raw_payload_bytes) {
      Ok(_) => {
        info!(
          "Webhook Pipeline: Payload for source '{}' parsed successfully as JSON.",
          source_id
        );
        Ok(PipelineControl::Continue)
      }
      Err(e) => {
        error!(
          "Webhook Pipeline: Failed to parse JSON payload for source '{}': {}",
          source_id, e
        );
        Err(AppError::Validation(format!("Invalid JSON payload: {}", e)))
      }
    }
  });

  p.conditional_scopes_for_step("route_webhook_event")
    .add_dynamic_scope(
      mock_payment_event_pipeline_factory,
      |main_ctx_data: ContextData<GenericWebhookCtxData>| {
        let raw_payload = main_ctx_data.read().raw_payload.clone();
        match serde_json::from_slice::<JsonValue>(&raw_payload) {
          Ok(json_value) => Ok(ContextData::new(json_value)),
          Err(e) => {
            error!("Extractor for payment_event: Failed to parse JSON: {}", e);
            Err(OrkaError::ExtractorFailure {
              step_name: "route_webhook_event_payment_extractor".to_string(),
              source: anyhow::Error::new(e).context("JSON parsing for payment event failed"),
            })
          }
        }
      },
    )
    .on_condition(|main_ctx_data: ContextData<GenericWebhookCtxData>| {
      let guard = main_ctx_data.read();
      if guard.source_identifier == "mock_payment_gateway" {
        if let Ok(json_value) = serde_json::from_slice::<JsonValue>(&guard.raw_payload) {
          return json_value.get("event_type").and_then(JsonValue::as_str) == Some("payment_succeeded");
        }
      }
      false
    })
    .if_no_scope_matches(PipelineControl::Continue)
    // Optional: unhandled webhook shapes are logged rather than failing the request.
    .finalize_conditional_step(true);

  p.on_root("acknowledge_webhook_receipt", |ctx_data| async move {
    let source_identifier = {
      let mut guard = ctx_data.write();
      guard.event_processed = true;
      guard.source_identifier.clone()
    };
    info!(
      "Webhook Pipeline: Event from source '{}' processed (or routed for processing). Ready to acknowledge.",
      source_identifier
    );
    Ok(PipelineControl::Continue)
  });

  orka_registry.register_pipeline(p)?;
  info!("Generic Webhook processing pipeline registered.");
  Ok(())
}
