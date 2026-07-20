use actix_web::{web, FromRequest, HttpRequest, HttpResponse};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pipelines::contexts::AddToCartCtxData;
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

/// Placeholder auth extractor. A real one would read a JWT or session rather than
/// trusting an `X-User-ID` header.
#[derive(Debug)]
pub struct AuthenticatedUser {
  pub user_id: Uuid,
}

impl FromRequest for AuthenticatedUser {
  type Error = AppError;
  type Future = futures_util::future::Ready<Result<Self, Self::Error>>;

  fn from_request(req: &HttpRequest, _payload: &mut actix_web::dev::Payload) -> Self::Future {
    if let Some(user_id_header) = req.headers().get("X-User-ID") {
      if let Ok(user_id_str) = user_id_header.to_str() {
        if let Ok(user_id) = Uuid::parse_str(user_id_str) {
          return futures_util::future::ready(Ok(AuthenticatedUser { user_id }));
        }
      }
    }
    warn!("AuthenticatedUser extractor: Missing or invalid X-User-ID header.");
    futures_util::future::ready(Err(AppError::Auth(
      "User authentication required. Missing or invalid X-User-ID header for mock auth.".to_string(),
    )))
  }
}

#[derive(Deserialize, Debug)]
pub struct AddToCartRequestPayload {
  pub product_id: Uuid,
  pub quantity: i32,
}

#[instrument(
    name = "handler::add_to_cart",
    skip(app_state, req_payload, auth_user),
    fields(user_id = %auth_user.user_id, product_id = %req_payload.product_id, quantity = %req_payload.quantity)
)]
pub async fn add_to_cart_handler(
  app_state: web::Data<AppState>,
  req_payload: web::Json<AddToCartRequestPayload>,
  auth_user: AuthenticatedUser,
) -> Result<HttpResponse, AppError> {
  info!(
    "Add to cart attempt by user: {}, product: {}, quantity: {}",
    auth_user.user_id, req_payload.product_id, req_payload.quantity
  );
  let add_to_cart_ctx_initial = AddToCartCtxData {
    app_state: app_state.get_ref().clone(),
    authenticated_user_id: auth_user.user_id,
    product_id: req_payload.product_id,
    quantity: req_payload.quantity,
    updated_cart_item: None,
  };
  let orka_context_data = ContextData::new(add_to_cart_ctx_initial);
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
      Ok(HttpResponse::Ok().json(json!({
          "message": "Item added to cart successfully.",
          "cartItem": updated_item
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
      warn!(
        "Add to Cart pipeline failed for user {}: {:?}",
        auth_user.user_id, app_err
      );
      Err(app_err)
    }
  }
}
