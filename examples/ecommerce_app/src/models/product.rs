// orka_project/examples/ecommerce_app/src/models/product.rs

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Product {
  pub id: Uuid,
  pub name: String,
  pub description: Option<String>, // Description can be optional
  pub price_cents: i32,
  pub stock_quantity: i32,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}
