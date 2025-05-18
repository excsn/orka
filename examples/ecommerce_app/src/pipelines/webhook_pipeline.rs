// orka_project/examples/ecommerce_app/src/pipelines/webhook_pipeline.rs

use crate::errors::AppError;
use crate::pipelines::contexts::GenericWebhookCtxData; // TData for this pipeline
                                                       // We might need more specific SData contexts for sub-pipelines triggered by webhooks
                                                       // e.g., PaymentWebhookEventCtxData, ShipmentUpdateEventCtxData
use crate::state::AppState;
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl}; // Orka for registration type hint
use serde_json::Value as JsonValue; // For parsing generic JSON
use std::sync::Arc;
use tracing::{error, info, instrument, warn, Level};

// Placeholder for a sub-pipeline factory for a specific event type
// In a real app, you'd have factories for different event types/sources
async fn mock_payment_event_pipeline_factory(
  _main_webhook_ctx: ContextData<GenericWebhookCtxData>, // May use data from main_webhook_ctx
                                                         // to initialize the sub-context
) -> Result<Arc<Pipeline<JsonValue, AppError>>, OrkaError> {
  // SData is JsonValue, Err is AppError
  info!("Factory: Creating mock payment event processing pipeline.");
  // This sub-pipeline would take the parsed JsonValue as its ContextData<JsonValue>
  // and process it, e.g., update order status.
  let mut p = Pipeline::<JsonValue, AppError>::new(&[("process_payment_success_event_detail", false, None)]);

  p.on_root(
    "process_payment_success_event_detail",
    |event_data_ctx: ContextData<JsonValue>| {
      Box::pin(async move {
        let event_data = event_data_ctx.read(); // event_data is &JsonValue
        info!("Mock Payment Event Sub-Pipeline: Processing event: {:?}", *event_data);
        // Example: Extract order ID and update status in DB
        // let order_id = event_data.get("order_id").and_then(JsonValue::as_str);
        // if let Some(id) = order_id {
        //   info!("Order ID from event: {}", id);
        //   // ... DB logic to update order status to 'paid' ...
        // } else {
        //   warn!("Order ID missing in payment event data.");
        //   return Err(AppError::Validation("Missing order_id in payment event.".to_string()));
        // }
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    },
  );
  Ok(Arc::new(p))
}

pub fn register_webhook_pipeline(orka_registry: &Arc<Orka<AppError>>, _app_state: &AppState) {
  // Pipeline is Pipeline<GenericWebhookCtxData, AppError>
  let mut p = Pipeline::<GenericWebhookCtxData, AppError>::new(&[
    ("verify_webhook_signature", true, None), // Optional, depends on provider
    ("parse_webhook_payload", false, None),
    ("route_webhook_event", false, None), // This step will use conditional scopes
    ("acknowledge_webhook_receipt", false, None), // Important to respond quickly
  ]);

  // Step 1: Verify Webhook Signature (Mocked)
  p.on_root(
    "verify_webhook_signature",
    |ctx_data: ContextData<GenericWebhookCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (source_id, signature_opt) = {
          let guard = ctx_data.read();
          (guard.source_identifier.clone(), guard.signature_header.clone())
        };

        info!(
          "Webhook Pipeline: Verifying signature for source '{}'. Signature provided: {}",
          source_id,
          signature_opt.is_some()
        );

        // Mock implementation:
        // In a real scenario, you would use the raw_payload (from ctx_data),
        // a secret key, and the signature algorithm specific to the provider.
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
    },
  );

  // Step 2: Parse Webhook Payload (as JSON)
  p.on_root(
    "parse_webhook_payload",
    |ctx_data: ContextData<GenericWebhookCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
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
          Ok(parsed_json) => {
            // If GenericWebhookCtxData had a field like `parsed_event_json: Option<JsonValue>`
            // {
            //    ctx_data.write().parsed_event_json = Some(parsed_json);
            // }
            // For conditional routing, we might put this parsed JSON into a temporary field
            // or the extractor for the conditional scope will parse it.
            // For simplicity, let's assume the extractor for the conditional scope will re-parse or use the raw payload.
            // Or, if the sub-pipeline directly takes JsonValue, the extractor will provide it.
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
      })
    },
  );

  // Step 3: Route Webhook Event (Conditional Scopes)
  // This step will determine the event type and dispatch to an appropriate sub-pipeline.
  // The sub-pipelines will operate on a more specific context, likely derived from parsing the JSON.
  p.conditional_scopes_for_step("route_webhook_event")
    // Example Scope 1: Handling a "payment_succeeded" event from "mock_payment_gateway"
    .add_dynamic_scope(
      mock_payment_event_pipeline_factory, // Factory returns Result<Arc<Pipeline<JsonValue, AppError>>, OrkaError>
      // Extractor: Takes GenericWebhookCtxData, returns Result<ContextData<JsonValue>, OrkaError>
      // It parses the raw payload into JsonValue for the sub-pipeline.
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
        // Further inspect raw_payload or a pre-parsed lightweight version
        // to determine if it's a 'payment_succeeded' type event.
        // This is simplified; real parsing might be needed earlier or be more robust.
        if let Ok(json_value) = serde_json::from_slice::<JsonValue>(&guard.raw_payload) {
          return json_value.get("event_type").and_then(JsonValue::as_str) == Some("payment_succeeded");
        }
      }
      false
    })
    // Add more scopes for other event types or sources
    // .add_dynamic_scope(...) / .add_static_scope(...)
    .if_no_scope_matches(PipelineControl::Continue) // Or Stop if unhandled events are errors
    .finalize_conditional_step(true); // Step is optional if some webhooks are just logged and not processed

  // Step 4: Acknowledge Webhook Receipt
  // This step should run quickly to send a 200 OK back to the webhook provider.
  // Actual processing of the event might happen in the sub-pipelines asynchronously.
  p.on_root(
    "acknowledge_webhook_receipt",
    |ctx_data: ContextData<GenericWebhookCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        // In a real Actix handler, you'd set the HTTP response status here.
        // Orka itself doesn't handle HTTP responses. This step is more about
        // marking in the context that the event has reached a point where
        // an acknowledgement can be sent.
        {
          let mut guard = ctx_data.write();
          guard.event_processed = true; // Or a more specific acknowledgement flag
        }
        info!(
          "Webhook Pipeline: Event from source '{}' processed (or routed for processing). Ready to acknowledge.",
          ctx_data.read().source_identifier
        );
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    },
  );

  orka_registry.register_pipeline(p);
  info!("Generic Webhook processing pipeline registered.");
}
