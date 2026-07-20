use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

// Line items are part of the example's schema but no route reads them back yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct OrderItem {
  pub id: Uuid,
  pub order_id: Uuid,
  pub product_id: Uuid,
  pub quantity: i32,
  pub price_at_purchase_cents: i32,
  // created_at/updated_at usually not needed for immutable line items
}
