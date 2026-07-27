use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
