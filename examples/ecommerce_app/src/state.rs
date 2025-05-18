// orka_project/examples/ecommerce_app/src/state.rs
use crate::config::AppConfig;
use crate::errors::AppError; // Or your specific error type for Orka
use sqlx::PgPool;
use std::sync::Arc; // Correct Brevo config

#[derive(Clone)]
pub struct AppState {
  pub db_pool: PgPool,
  pub orka_instance: Arc<orka::Orka<AppError>>,
  pub config: Arc<AppConfig>, // Share loaded config
}
