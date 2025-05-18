// orka_project/examples/ecommerce_app/src/models/order.rs

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, Type as SqlxType};
use uuid::Uuid; // Renamed Type to SqlxType to avoid conflict

// Renamed from order_status to order_status_enum to match schema.sql
// and avoid potential conflicts.
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

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Order {
  pub id: Uuid,
  pub user_id: Uuid,
  pub status: OrderStatus,
  pub total_amount_cents: i32,
  pub currency: String,
  // These were Stripe-specific, can be generalized or made optional if needed by mocks
  pub payment_gateway_txn_id: Option<String>,      // Generic transaction ID
  pub payment_gateway_client_data: Option<String>, // Generic client data (like client_secret)
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}
