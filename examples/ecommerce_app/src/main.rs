mod config;
mod errors;
mod models;
mod pipelines;
mod services;
mod state;
mod web;

use crate::config::AppConfig;
use crate::errors::AppError;
use crate::state::AppState;

use actix_web::{web as actix_data, App, HttpServer};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
  tracing_subscriber::fmt()
    .with_max_level(Level::INFO)
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .with_span_events(FmtSpan::CLOSE)
    .init();

  tracing::info!("Starting e-commerce application server...");

  let app_config = match AppConfig::from_env() {
    Ok(cfg) => Arc::new(cfg),
    Err(e) => {
      tracing::error!(error = %e, "Failed to load application configuration.");
      panic!("Configuration error: {}", e);
    }
  };

  let db_pool = match PgPool::connect(&app_config.database_url).await {
    Ok(pool) => {
      tracing::info!("Successfully connected to the database.");
      pool
    }
    Err(e) => {
      tracing::error!(error = %e, "Failed to connect to the database.");
      panic!("Database connection error: {}", e);
    }
  };

  if app_config.seed_db {
    tracing::info!("Database seeding enabled (implement seed_db function).");
  }

  // Orka<AppError> so that Orka::run surfaces this app's own error type.
  let orka_instance = Arc::new(orka::Orka::<AppError>::new());

  let app_state = AppState {
    db_pool: db_pool.clone(),
    orka_instance: orka_instance.clone(),
    config: app_config.clone(),
  };

  pipelines::register_all_pipelines(&orka_instance, &app_state).expect("Failed to register Orka pipelines");

  let server_address = format!("{}:{}", app_config.server_host, app_config.server_port);
  tracing::info!("Attempting to bind server to {}...", server_address);

  HttpServer::new(move || {
    App::new()
      .app_data(actix_data::Data::new(app_state.clone()))
      .wrap(tracing_actix_web::TracingLogger::default())
      .configure(web::configure_app_routes)
  })
  .bind(&server_address)?
  .run()
  .await
}
