// examples/ecommerce_app/src/web/handlers/product_handlers.rs

use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;
use serde_json::json;
// use sqlx::Row; // Not needed if using query_as directly with struct that impl FromRow
use tracing::{error, info, instrument, warn, Level};
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::product::Product;
use crate::state::AppState;

#[derive(Deserialize, Debug)]
pub struct ListProductsQuery {
  // pub page: Option<i64>,
  // pub limit: Option<i64>,
}

#[instrument(name = "handler::list_products", skip(app_state, query_params))]
pub async fn list_products_handler(
  app_state: web::Data<AppState>,
  query_params: web::Query<ListProductsQuery>,
) -> Result<HttpResponse, AppError> {
  info!("Attempting to list products (using runtime queries).");

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

#[instrument(name = "handler::get_product", skip(app_state, path), fields(product_id = %path.as_ref()))] // Use .as_ref() for logging Path content
pub async fn get_product_handler(
  app_state: web::Data<AppState>,
  path: web::Path<Uuid>, // path is of type Path<Uuid>
) -> Result<HttpResponse, AppError> {
  // Correct way to get the inner Uuid value from web::Path
  let product_id_to_fetch = path.into_inner(); // THIS LINE IS CORRECTED

  info!(
    "Attempting to fetch product with ID: {} (using runtime queries).",
    product_id_to_fetch
  );

  let product_opt: Option<Product> = sqlx::query_as(
    "SELECT id, name, description, price_cents, stock_quantity, created_at, updated_at FROM products WHERE id = $1",
  )
  .bind(product_id_to_fetch) // Use the extracted Uuid
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
