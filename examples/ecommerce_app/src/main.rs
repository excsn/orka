// orka_project/examples/ecommerce_app/src/main.rs

// Declare modules for the application
mod config;
mod errors;
mod models;
// mod db; // Uncomment if you create the db module
mod pipelines;
mod services;
mod state;
mod web;

use crate::config::AppConfig;
use crate::errors::{AppError, Result as AppResult}; // Use the app's Result alias
use crate::state::AppState;

use actix_web::{web as actix_data, App, HttpServer}; // Renamed web to actix_data
use sqlx::PgPool;
use std::sync::Arc;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan; // For span events in tracing

// Main function
#[actix_web::main]
async fn main() -> std::io::Result<()> {
  // Initialize tracing subscriber for logging
  // (Customize as needed, e.g., with JSON output, OpenTelemetry)
  tracing_subscriber::fmt()
    .with_max_level(Level::INFO) // Default level
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()) // Allow RUST_LOG override
    .with_span_events(FmtSpan::CLOSE) // Log when spans close, showing duration
    .init();

  tracing::info!("Starting e-commerce application server...");

  // Load application configuration
  let app_config = match AppConfig::from_env() {
    Ok(cfg) => Arc::new(cfg), // Arc the config for sharing
    Err(e) => {
      tracing::error!(error = %e, "Failed to load application configuration.");
      // For a simple example, panic is okay. In prod, might exit gracefully.
      panic!("Configuration error: {}", e);
    }
  };

  // Initialize Database Pool
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

  // Seed database if configured
  if app_config.seed_db {
    // Placeholder for seed function
    // if let Err(e) = seed_db(&db_pool).await {
    //     tracing::error!(error = %e, "Failed to seed database.");
    // }
    tracing::info!("Database seeding enabled (implement seed_db function).");
  }

  // Initialize Orka instance
  // Orka<AppError> because we want Orka::run to return our AppError
  let orka_instance = Arc::new(orka::Orka::<AppError>::new());

  // Create AppState
  let app_state = AppState {
    db_pool: db_pool.clone(),
    orka_instance: orka_instance.clone(), // Clone Arc for AppState
    config: app_config.clone(),           // Clone Arc for AppState
  };

  // Register all Orka pipelines
  // This function will live in `pipelines/mod.rs` or be composed of calls to other pipeline modules.
  pipelines::register_all_pipelines(&orka_instance, &app_state);
  tracing::info!("Orka pipelines registered.");

  // Configure and Start Actix Web Server
  let server_address = format!("{}:{}", app_config.server_host, app_config.server_port);
  tracing::info!("Attempting to bind server to {}...", server_address);

  HttpServer::new(move || {
    App::new()
      .app_data(actix_data::Data::new(app_state.clone())) // Share AppState with handlers
      .wrap(tracing_actix_web::TracingLogger::default()) // Actix middleware for tracing requests
  })
  .bind(&server_address)?
  .run()
  .await
}
