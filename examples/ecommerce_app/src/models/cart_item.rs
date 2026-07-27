use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CartItem {
  pub id: Uuid,
  pub user_id: Uuid,
  pub product_id: Uuid,
  pub quantity: i32,
  pub added_at: DateTime<Utc>,
  // pub updated_at: DateTime<Utc>,
}
