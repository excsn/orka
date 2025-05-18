// examples/ecommerce_app/src/web/handlers/cart_handlers.rs

use actix_web::{web, FromRequest, HttpRequest, HttpResponse, Responder};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, instrument, warn, Level};
use uuid::Uuid; // For Uuid type

use crate::errors::AppError;
use crate::pipelines::contexts::AddToCartCtxData;
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

// --- Custom Extractor for Authenticated User (Placeholder) ---
// In a real application, this would be implemented to extract user identity
// from a JWT, session, or other authentication mechanism.
#[derive(Debug)]
pub struct AuthenticatedUser {
  pub user_id: Uuid,
}

// Minimal implementation for the example.
// A real implementation would involve async logic and error handling.
impl FromRequest for AuthenticatedUser {
  type Error = AppError; // Use your app's error type
  type Future = futures_util::future::Ready<Result<Self, Self::Error>>;

  fn from_request(req: &HttpRequest, _payload: &mut actix_web::dev::Payload) -> Self::Future {
    // Placeholder: In a real app, extract from token/session.
    // For this example, let's try to get a user_id from a header for testing.
    // Or, for simplicity in this mock, just return a fixed Uuid or error if not found.
    if let Some(user_id_header) = req.headers().get("X-User-ID") {
      if let Ok(user_id_str) = user_id_header.to_str() {
        if let Ok(user_id) = Uuid::parse_str(user_id_str) {
          return futures_util::future::ready(Ok(AuthenticatedUser { user_id }));
        }
      }
    }
    // If no valid X-User-ID header, return an authentication error.
    warn!("AuthenticatedUser extractor: Missing or invalid X-User-ID header.");
    futures_util::future::ready(Err(AppError::Auth(
      "User authentication required. Missing or invalid X-User-ID header for mock auth.".to_string(),
    )))
  }
}

// --- Request DTO ---
#[derive(Deserialize, Debug)]
pub struct AddToCartRequestPayload {
  pub product_id: Uuid,
  pub quantity: i32,
}

// --- Handler Implementation ---

#[instrument(
    name = "handler::add_to_cart",
    skip(app_state, req_payload, auth_user),
    fields(user_id = %auth_user.user_id, product_id = %req_payload.product_id, quantity = %req_payload.quantity)
)]
pub async fn add_to_cart_handler(
  app_state: web::Data<AppState>,
  req_payload: web::Json<AddToCartRequestPayload>,
  auth_user: AuthenticatedUser, // Extracted authenticated user
) -> Result<HttpResponse, AppError> {
  info!(
    "Add to cart attempt by user: {}, product: {}, quantity: {}",
    auth_user.user_id, req_payload.product_id, req_payload.quantity
  );

  // 1. Prepare the initial context data for the add_to_cart pipeline
  let add_to_cart_ctx_initial = AddToCartCtxData {
    app_state: app_state.get_ref().clone(),
    authenticated_user_id: auth_user.user_id,
    product_id: req_payload.product_id,
    quantity: req_payload.quantity,
    updated_cart_item: None, // This will be populated by the pipeline
  };
  let orka_context_data = ContextData::new(add_to_cart_ctx_initial);

  // 2. Run the add_to_cart pipeline
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
      let final_ctx_guard = orka_context_data.read();
      let updated_item = final_ctx_guard.updated_cart_item.as_ref().ok_or_else(|| {
        warn!(
          "Add to Cart pipeline completed for user {} but updated_cart_item was not set.",
          auth_user.user_id
        );
        AppError::Internal("Cart update completed, but item details are unavailable.".to_string())
      })?;

      info!(
        "Add to cart successful for user: {}. Item ID: {}, Product ID: {}, New Quantity: {}",
        auth_user.user_id, updated_item.id, updated_item.product_id, updated_item.quantity
      );

      // 3. Construct and return HTTP response
      Ok(HttpResponse::Ok().json(json!({
          "message": "Item added to cart successfully.",
          "cartItem": updated_item // Serialize the CartItem model
      })))
    }
    Ok(PipelineResult::Stopped) => {
      warn!(
        "Add to Cart pipeline for user {} was stopped by a handler.",
        auth_user.user_id
      );
      Err(AppError::Internal(
        "Process to add item to cart was halted.".to_string(),
      ))
    }
    Err(app_err) => {
      // Handles AppError::Validation("Insufficient stock"), AppError::NotFound("Product not found"), etc.
      warn!(
        "Add to Cart pipeline failed for user {}: {:?}",
        auth_user.user_id, app_err
      );
      Err(app_err)
    }
  }
}
