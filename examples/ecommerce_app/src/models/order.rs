use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, Type as SqlxType};
use uuid::Uuid;

// Orders are part of the example's schema but no route reads them back yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, SqlxType)]
#[sqlx(type_name = "order_status_enum", rename_all = "lowercase")]
pub enum OrderStatus {
  Pending,
  PaymentDue,
  Paid,
  Failed,
  Shipped,
  Delivered,
  Cancelled,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Order {
  pub id: Uuid,
  pub user_id: Uuid,
  pub status: OrderStatus,
  pub total_amount_cents: i32,
  pub currency: String,
  pub payment_gateway_txn_id: Option<String>,
  pub payment_gateway_client_data: Option<String>,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}
