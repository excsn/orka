// orka_project/examples/ecommerce_app/src/models/order_item.rs

use serde::Serialize; // Add Deserialize if you construct OrderItems from e.g. cart data requests
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)] // Add Deserialize if needed
pub struct OrderItem {
  pub id: Uuid,
  pub order_id: Uuid,
  pub product_id: Uuid,
  pub quantity: i32,
  pub price_at_purchase_cents: i32,
  // created_at/updated_at usually not needed for immutable line items
}
