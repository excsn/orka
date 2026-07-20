use actix_web::{web, HttpResponse};
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::product::Product;
use crate::state::AppState;

#[derive(Deserialize, Debug)]
pub struct ListProductsQuery {}

#[instrument(name = "handler::list_products", skip_all)]
pub async fn list_products_handler(
  app_state: web::Data<AppState>,
  _query_params: web::Query<ListProductsQuery>,
) -> Result<HttpResponse, AppError> {
  info!("Attempting to list products.");

  let products: Vec<Product> = sqlx::query_as(
    "SELECT id, name, description, price_cents, stock_quantity, created_at, updated_at FROM products ORDER BY name ASC",
  )
  .fetch_all(&app_state.db_pool)
  .await
  .map_err(|e| {
    error!("Failed to fetch products from database: {}", e);
    AppError::Sqlx(e)
  })?;

  info!("Successfully fetched {} products.", products.len());

  Ok(HttpResponse::Ok().json(json!({
      "message": "Products fetched successfully.",
      "products": products
  })))
}

#[instrument(name = "handler::get_product", skip(app_state, path), fields(product_id = %path.as_ref()))]
pub async fn get_product_handler(
  app_state: web::Data<AppState>,
  path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
  let product_id_to_fetch = path.into_inner();

  info!(
    "Attempting to fetch product with ID: {}.",
    product_id_to_fetch
  );

  let product_opt: Option<Product> = sqlx::query_as(
    "SELECT id, name, description, price_cents, stock_quantity, created_at, updated_at FROM products WHERE id = $1",
  )
  .bind(product_id_to_fetch)
  .fetch_optional(&app_state.db_pool)
  .await
  .map_err(|e| {
    error!("Database error while fetching product {}: {}", product_id_to_fetch, e);
    AppError::Sqlx(e)
  })?;

  match product_opt {
    Some(product) => {
      info!("Product {} fetched successfully.", product_id_to_fetch);
      Ok(HttpResponse::Ok().json(json!({
          "message": "Product fetched successfully.",
          "product": product
      })))
    }
    None => {
      warn!("Product with ID {} not found.", product_id_to_fetch);
      Err(AppError::NotFound(format!(
        "Product with ID {} not found.",
        product_id_to_fetch
      )))
    }
  }
}
