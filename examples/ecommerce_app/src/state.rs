use crate::config::AppConfig;
use crate::errors::AppError;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
  pub db_pool: PgPool,
  pub orka_instance: Arc<orka::Orka<AppError>>,
  pub config: Arc<AppConfig>,
}
