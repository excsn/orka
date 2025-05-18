// orka_project/examples/ecommerce_app/src/models/cart_item.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize}; // Deserialize for request body
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CartItem {
  pub id: Uuid, // Primary key for the cart_item itself
  pub user_id: Uuid,
  pub product_id: Uuid,
  pub quantity: i32,
  pub added_at: DateTime<Utc>,
  // updated_at might be useful if quantity can be updated in place.
  // pub updated_at: DateTime<Utc>,
}
