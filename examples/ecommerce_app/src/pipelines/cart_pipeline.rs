// orka_project/examples/ecommerce_app/src/pipelines/cart_pipeline.rs

use crate::errors::AppError;
use crate::models::{cart_item::CartItem, product::Product};
use crate::pipelines::contexts::AddToCartCtxData;
use crate::state::AppState;
use orka::{ContextData, Orka, Pipeline, PipelineControl}; // Orka is for registration type hint
use std::sync::Arc;
use tracing::{error, info, instrument, warn, Level};
use uuid::Uuid;

pub fn register_add_to_cart_pipeline(orka_registry: &Arc<Orka<AppError>>, _app_state: &AppState) {
  // Pipeline is Pipeline<AddToCartCtxData, AppError>
  let mut p = Pipeline::<AddToCartCtxData, AppError>::new(&[
    ("validate_cart_input", false, None),
    ("fetch_product_for_cart", false, None),
    ("check_product_stock_for_cart", false, None),
    ("add_or_update_cart_item_db", false, None),
    // ("update_product_stock_post_cart_add", false, None), // Potentially a separate, more robust step/pipeline
  ]);

  // Step 1: Validate input (product_id from context, quantity from context)
  p.on_root("validate_cart_input", |ctx_data: ContextData<AddToCartCtxData>| {
    Box::pin(async move {
      // Returns Result<PipelineControl, AppError>
      let quantity = { ctx_data.read().quantity }; // Read quantity

      if quantity <= 0 {
        warn!(
          "Add to Cart Pipeline: Invalid quantity ({}) provided. Must be positive.",
          quantity
        );
        return Err(AppError::Validation("Quantity must be a positive number.".to_string()));
      }
      // product_id is Uuid, its format validity is handled by request parsing.
      // We'll validate its existence in the DB in the next step.
      info!("Add to Cart Pipeline: Input quantity ({}) validated.", quantity);
      Ok(PipelineControl::Continue)
    })
  });

  // Step 2: Fetch product details
  p.on_root("fetch_product_for_cart", |ctx_data: ContextData<AddToCartCtxData>| {
    Box::pin(async move {
      // Returns Result<PipelineControl, AppError>
      let (product_id_to_fetch, db_pool) = {
        let guard = ctx_data.read();
        (guard.product_id, guard.app_state.db_pool.clone())
      };

      info!(
        "Add to Cart Pipeline: Fetching product details for product_id: {}",
        product_id_to_fetch
      );

      // --- Placeholder DB Operation: Fetch product ---
      // match sqlx::query_as!(Product, "SELECT * FROM products WHERE id = $1", product_id_to_fetch)
      //   .fetch_optional(&db_pool)
      //   .await
      // {
      //   Ok(Some(product)) => {
      //     // Store fetched product details in context if needed by subsequent steps,
      //     // e.g., price or if stock check needs more than just quantity.
      //     // For now, we assume stock check only needs product.stock_quantity.
      //     // {
      //     //    let mut guard = ctx_data.write();
      //     //    guard.fetched_product = Some(product); // If AddToCartCtxData had this field
      //     // }
      //     info!("Add to Cart Pipeline: Product {} found. Price: {}, Stock: {}", product.id, product.price_cents, product.stock_quantity);
      //     Ok(PipelineControl::Continue)
      //   }
      //   Ok(None) => {
      //     warn!("Add to Cart Pipeline: Product {} not found.", product_id_to_fetch);
      //     Err(AppError::NotFound(format!("Product with ID {} not found.", product_id_to_fetch)))
      //   }
      //   Err(e) => {
      //     error!("Add to Cart Pipeline: DB error fetching product {}: {}", product_id_to_fetch, e);
      //     Err(AppError::Sqlx(e))
      //   }
      // }
      // For materialized example:
      info!(
        "Add to Cart Pipeline: Simulated product fetch for {}.",
        product_id_to_fetch
      );
      // Pretend product is fetched and store its stock for next step if needed.
      // If AddToCartCtxData had a field like `fetched_product_stock: Option<i32>`:
      // { ctx_data.write().fetched_product_stock = Some(10); } // Example stock
      Ok::<_, AppError>(PipelineControl::Continue)
    })
  });

  // Step 3: Check product stock
  p.on_root(
    "check_product_stock_for_cart",
    |ctx_data: ContextData<AddToCartCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (requested_quantity, product_id_for_stock_check, db_pool) = {
          let guard = ctx_data.read();
          (guard.quantity, guard.product_id, guard.app_state.db_pool.clone())
        };

        info!(
          "Add to Cart Pipeline: Checking stock for product_id: {}, requested: {}",
          product_id_for_stock_check, requested_quantity
        );

        // --- Placeholder DB Operation: Get current stock ---
        // This would typically fetch the current stock_quantity for the product_id.
        // let current_stock: i32 = sqlx::query_scalar!("SELECT stock_quantity FROM products WHERE id = $1", product_id_for_stock_check)
        //     .fetch_one(&db_pool)
        //     .await
        //     .map_err(|e| {
        //         if matches!(e, sqlx::Error::RowNotFound) {
        //             AppError::NotFound(format!("Product {} not found during stock check.", product_id_for_stock_check))
        //         } else {
        //             AppError::Sqlx(e)
        //         }
        //     })?;
        //
        // if current_stock < requested_quantity {
        //   warn!(
        //     "Add to Cart Pipeline: Insufficient stock for product {}. Available: {}, Requested: {}.",
        //     product_id_for_stock_check, current_stock, requested_quantity
        //   );
        //   return Err(AppError::Validation(format!(
        //     "Insufficient stock for product. Only {} available.",
        //     current_stock
        //   )));
        // }
        // For materialized example:
        let current_stock = 10; // Assume fetched stock
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
    },
  );

  // Step 4: Add or Update item in user's cart
  p.on_root(
    "add_or_update_cart_item_db",
    |ctx_data: ContextData<AddToCartCtxData>| {
      Box::pin(async move {
        // Returns Result<PipelineControl, AppError>
        let (user_id, product_id, quantity, db_pool) = {
          let guard = ctx_data.read();
          (
            guard.authenticated_user_id,
            guard.product_id,
            guard.quantity,
            guard.app_state.db_pool.clone(),
          )
        };

        info!(
          "Add to Cart Pipeline: Adding/updating cart for user {}, product {}, quantity {}.",
          user_id, product_id, quantity
        );

        // --- Placeholder DB Operation: UPSERT logic for cart_items ---
        // This involves checking if the (user_id, product_id) pair exists.
        // If yes, UPDATE quantity. If no, INSERT new row.
        // Example simplified UPSERT for PostgreSQL:
        // let cart_item_id = Uuid::new_v4(); // Generate new ID for potential insert
        // let result = sqlx::query_as!(
        //   CartItem,
        //   r#"
        //   INSERT INTO cart_items (id, user_id, product_id, quantity, added_at)
        //   VALUES ($1, $2, $3, $4, NOW())
        //   ON CONFLICT (user_id, product_id) DO UPDATE
        //   SET quantity = cart_items.quantity + $4, added_at = NOW() -- Or just SET quantity = EXCLUDED.quantity for override
        //   RETURNING *
        //   "#,
        //   cart_item_id, // Used if new row
        //   user_id,
        //   product_id,
        //   quantity
        // )
        // .fetch_one(&db_pool)
        // .await;
        //
        // match result {
        //   Ok(updated_cart_item) => {
        //     info!(
        //       "Add to Cart Pipeline: Cart item {} (product {}) for user {} updated/added. New quantity: {}",
        //       updated_cart_item.id, product_id, user_id, updated_cart_item.quantity
        //     );
        //     {
        //       ctx_data.write().updated_cart_item = Some(updated_cart_item);
        //     }
        //     Ok(PipelineControl::Continue)
        //   }
        //   Err(e) => {
        //     error!(
        //       "Add to Cart Pipeline: DB error adding/updating cart item for user {}, product {}: {}",
        //       user_id, product_id, e
        //     );
        //     Err(AppError::Sqlx(e))
        //   }
        // }
        // For materialized example:
        let updated_cart_item_mock = CartItem {
          id: Uuid::new_v4(),
          user_id,
          product_id,
          quantity, // Assume new quantity after update/add
          added_at: chrono::Utc::now(),
        };
        info!(
          "Add to Cart Pipeline: Simulated cart item add/update for user {}, product {}.",
          user_id, product_id
        );
        {
          ctx_data.write().updated_cart_item = Some(updated_cart_item_mock);
        }
        Ok::<_, AppError>(PipelineControl::Continue)
      })
    },
  );

  // Step 5 (Optional & Complex): Update product stock
  // This is often a tricky part due to concurrency.
  // A simple approach is to decrement stock here. A more robust system might
  // use reservations, events, or a dedicated inventory service/pipeline.
  // For now, let's comment it out as it adds significant complexity if done properly.
  /*
  p.on_root(
    "update_product_stock_post_cart_add",
    |ctx_data: ContextData<AddToCartCtxData>| {
      Box::pin(async move { // Returns Result<PipelineControl, AppError>
        let (product_id, quantity_added, db_pool) = {
          let guard = ctx_data.read();
          (
            guard.product_id,
            guard.quantity, // This is the quantity *added* in this operation
            guard.app_state.db_pool.clone(),
          )
        };

        info!(
          "Add to Cart Pipeline: Updating stock for product_id: {} by -{}",
          product_id, quantity_added
        );

        // --- Placeholder DB Operation: Decrement stock ---
        // IMPORTANT: This must be done carefully to avoid race conditions and negative stock.
        // Often `UPDATE products SET stock_quantity = stock_quantity - $1 WHERE id = $2 AND stock_quantity >= $1`
        // And check rows_affected.
        // let result = sqlx::query!(
        //   "UPDATE products SET stock_quantity = stock_quantity - $1 WHERE id = $2 AND stock_quantity >= $1",
        //   quantity_added,
        //   product_id
        // )
        // .execute(&db_pool)
        // .await;
        //
        // match result {
        //    Ok(exec_result) => {
        //        if exec_result.rows_affected() == 1 {
        //            info!("Add to Cart Pipeline: Stock updated for product {}.", product_id);
        //            Ok(PipelineControl::Continue)
        //        } else {
        //            // This means stock became insufficient between check and update, or product disappeared.
        //            // This is a critical race condition or data integrity issue.
        //            // The application needs a strategy: rollback cart add, error out, etc.
        //            warn!("Add to Cart Pipeline: Failed to update stock for product {} (race condition or product gone?). Rolling back may be needed.", product_id);
        //            // Forcing a stop and potentially an error that leads to rollback in a transactional handler
        //            Err(AppError::Internal(format!("Stock update failed for product {}, possibly due to concurrent modification.", product_id)))
        //        }
        //    }
        //    Err(e) => {
        //        error!("Add to Cart Pipeline: DB error updating stock for product {}: {}", product_id, e);
        //        Err(AppError::Sqlx(e))
        //    }
        // }
        info!("Add to Cart Pipeline: Simulated stock update for product {}.", product_id);
        Ok(PipelineControl::Continue)
      })
    },
  );
  */

  orka_registry.register_pipeline(p);
  info!("Add to Cart pipeline registered.");
}
