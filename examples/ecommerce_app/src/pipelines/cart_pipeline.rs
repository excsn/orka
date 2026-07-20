use crate::errors::AppError;
use crate::models::cart_item::CartItem;
use crate::pipelines::contexts::AddToCartCtxData;
use crate::state::AppState;
use orka::{Orka, OrkaResult, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

pub fn register_add_to_cart_pipeline(orka_registry: &Arc<Orka<AppError>>, _app_state: &AppState) -> OrkaResult<()> {
  let mut p = Pipeline::<AddToCartCtxData, AppError>::new([
    "validate_cart_input",
    "fetch_product_for_cart",
    "check_product_stock_for_cart",
    "add_or_update_cart_item_db",
  ]);

  p.on_root("validate_cart_input", |ctx_data| async move {
    let quantity = { ctx_data.read().quantity };

    if quantity <= 0 {
      warn!(
        "Add to Cart Pipeline: Invalid quantity ({}) provided. Must be positive.",
        quantity
      );
      return Err(AppError::Validation("Quantity must be a positive number.".to_string()));
    }
    info!("Add to Cart Pipeline: Input quantity ({}) validated.", quantity);
    Ok(PipelineControl::Continue)
  })
  .on_root("fetch_product_for_cart", |ctx_data| async move {
    let product_id_to_fetch = { ctx_data.read().product_id };

    info!(
      "Add to Cart Pipeline: Simulated product fetch for {}.",
      product_id_to_fetch
    );
    Ok(PipelineControl::Continue)
  })
  .on_root("check_product_stock_for_cart", |ctx_data| async move {
    let (requested_quantity, product_id_for_stock_check) = {
      let guard = ctx_data.read();
      (guard.quantity, guard.product_id)
    };

    // Stands in for a `SELECT stock_quantity FROM products WHERE id = $1`.
    let current_stock = 10;
    if current_stock < requested_quantity {
      warn!(
        "Add to Cart Pipeline: Insufficient stock for product {}. Available: {}, Requested: {}.",
        product_id_for_stock_check, current_stock, requested_quantity
      );
      return Err(AppError::Validation(format!(
        "Insufficient stock. Only {} available.",
        current_stock
      )));
    }

    info!(
      "Add to Cart Pipeline: Stock sufficient for product {}. Available: {}, Requested: {}.",
      product_id_for_stock_check, current_stock, requested_quantity
    );
    Ok(PipelineControl::Continue)
  })
  .on_root("add_or_update_cart_item_db", |ctx_data| async move {
    let (user_id, product_id, quantity) = {
      let guard = ctx_data.read();
      (guard.authenticated_user_id, guard.product_id, guard.quantity)
    };

    // Stands in for an UPSERT on cart_items keyed by (user_id, product_id).
    let updated_cart_item_mock = CartItem {
      id: Uuid::new_v4(),
      user_id,
      product_id,
      quantity,
      added_at: chrono::Utc::now(),
    };
    info!(
      "Add to Cart Pipeline: Simulated cart item add/update for user {}, product {}.",
      user_id, product_id
    );
    ctx_data.write().updated_cart_item = Some(updated_cart_item_mock);
    Ok(PipelineControl::Continue)
  });

  orka_registry.register_pipeline(p)?;
  info!("Add to Cart pipeline registered.");
  Ok(())
}
