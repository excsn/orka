// orka_project/examples/ecommerce_app/src/models/user.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize}; // Deserialize might be needed for request bodies if not directly for DB
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
  pub id: Uuid,
  pub email: String,
  #[serde(skip_serializing)] // Never send password hash to client
  pub password_hash: String,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}
