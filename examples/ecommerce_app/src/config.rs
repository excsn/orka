use crate::errors::{AppError, Result};
use dotenvy::dotenv;
use std::env;

#[derive(Debug, Clone)]
#[allow(dead_code)] // app_base_url is loaded for completeness; no route needs it yet.
pub struct AppConfig {
  pub server_host: String,
  pub server_port: u16,
  pub database_url: String,
  pub app_base_url: String,

  pub mock_payment_provider_main_id: String,
  pub mock_payment_provider_alt_id: String,
  pub mock_email_sender: String,

  pub seed_db: bool,
}

impl AppConfig {
  pub fn from_env() -> Result<Self> {
    dotenv().ok();

    let get_env = |var_name: &str| {
      env::var(var_name).map_err(|e| AppError::Config(format!("Missing environment variable '{}': {}", var_name, e)))
    };

    let server_host = get_env("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let server_port = get_env("SERVER_PORT")
      .unwrap_or_else(|_| "8080".to_string())
      .parse::<u16>()
      .map_err(|e| AppError::Config(format!("Invalid SERVER_PORT: {}", e)))?;
    let database_url = get_env("DATABASE_URL")?;
    let app_base_url = get_env("APP_BASE_URL").unwrap_or_else(|_| format!("http://{}:{}", server_host, server_port));

    let mock_payment_provider_main_id =
      get_env("MOCK_PAYMENT_MAIN_ID").unwrap_or_else(|_| "mock_main_acct".to_string());
    let mock_payment_provider_alt_id = get_env("MOCK_PAYMENT_ALT_ID").unwrap_or_else(|_| "mock_alt_acct".to_string());
    let mock_email_sender = get_env("MOCK_EMAIL_SENDER").unwrap_or_else(|_| "noreply@example.com".to_string());

    let seed_db = get_env("SEED_DB")
      .unwrap_or_else(|_| "false".to_string())
      .parse::<bool>()
      .map_err(|e| AppError::Config(format!("Invalid SEED_DB value: {}", e)))?;

    tracing::info!("Application configuration loaded successfully.");

    Ok(Self {
      server_host,
      server_port,
      database_url,
      app_base_url,
      mock_payment_provider_main_id,
      mock_payment_provider_alt_id,
      mock_email_sender,
      seed_db,
    })
  }
}
