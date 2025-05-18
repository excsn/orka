// orka_project/examples/ecommerce_app/src/errors.rs

use actix_web::{HttpResponse, ResponseError};
use serde_json::json;
use thiserror::Error;

// Import Orka's error type. Assuming orka::Error is the public alias for orka::OrkaError
// or directly use orka::OrkaError if that's the one re-exported or preferred.
use orka::OrkaError; // Or use the specific type if `orka::Error` isn't the alias for OrkaError

#[derive(Debug, Error)]
pub enum AppError {
  #[error("Validation Error: {0}")]
  Validation(String),

  #[error("Authentication Failed: {0}")]
  Auth(String),

  #[error("Resource Not Found: {0}")]
  NotFound(String),

  #[error("Payment Processing Error: {0}")]
  Payment(String), // For generic payment issues not from Stripe directly

  #[error("Configuration Error: {0}")]
  Config(String),

  #[error("Database Error: {0}")]
  Sqlx(#[from] sqlx::Error),

  #[error("Stripe API Error: {0}")]
  Stripe(String), // Store Stripe error as string for simplicity with ResponseError

  #[error("Brevo Email Error: {0}")]
  Brevo(String), // Store Brevo error as string

  #[error("Orka Workflow Error: {source}")]
  Workflow {
    #[from] // Allows conversion from orka::OrkaError
    source: OrkaError,
  },

  #[error("Internal Server Error: {0}")]
  Internal(String), // For miscellaneous errors

  // This can be used by HTTP handlers if an Orka pipeline stops gracefully
  // but the HTTP handler considers it an error condition for the request.
  #[error("Pipeline execution was halted by a handler.")]
  PipelineHaltedByHandler,
}

// Allow anyhow::Error to be converted into AppError::Internal for convenience in handlers
// This is useful if handlers use `?` on functions returning anyhow::Result
impl From<anyhow::Error> for AppError {
  fn from(err: anyhow::Error) -> Self {
    // Attempt to downcast to a more specific AppError first if anyhow was wrapping one
    if let Some(app_err) = err.downcast_ref::<AppError>() {
      // This requires AppError to be Clone. If not, we can't return *app_err directly.
      // Let's reconstruct a similar error for simplicity, or make AppError Clone.
      // For now, just convert to internal.
      return AppError::Internal(format!("Downcasted AppError: {}", app_err.to_string()));
    }
    // Check for other common error types that might be wrapped in anyhow
    if err.is::<sqlx::Error>() {
      // We already have `From<sqlx::Error>`, but this handles if it was wrapped in anyhow
      return AppError::Sqlx(err.downcast::<sqlx::Error>().unwrap());
    }
    // Add other specific downcasts if needed

    AppError::Internal(err.to_string())
  }
}

impl ResponseError for AppError {
  fn error_response(&self) -> HttpResponse {
    // Log the full error when it's turned into a response
    tracing::error!(application_error = %self, "Responding with error");
    match self {
      AppError::Validation(m) => HttpResponse::BadRequest().json(json!({"error": m})),
      AppError::Auth(m) => HttpResponse::Unauthorized().json(json!({"error": m})),
      AppError::NotFound(m) => HttpResponse::NotFound().json(json!({"error": m})),
      AppError::Payment(m) => HttpResponse::PaymentRequired().json(json!({"error": m})),
      AppError::Config(m) => {
        HttpResponse::InternalServerError().json(json!({"error": "Configuration issue", "detail": m}))
      }
      AppError::Sqlx(_) => HttpResponse::InternalServerError().json(json!({"error": "Database operation failed"})),
      AppError::Stripe(m) => {
        HttpResponse::InternalServerError().json(json!({"error": "Payment provider error", "detail": m}))
      }
      AppError::Brevo(m) => {
        HttpResponse::InternalServerError().json(json!({"error": "Email service error", "detail": m}))
      }
      AppError::Workflow { source } => {
        // You might want to inspect `source` (OrkaError) for more specific HTTP responses
        tracing::error!(orka_error_source = ?source, "Workflow error details");
        HttpResponse::InternalServerError()
          .json(json!({"error": "Workflow processing error", "detail": source.to_string()}))
      }
      AppError::Internal(m) => {
        HttpResponse::InternalServerError().json(json!({"error": "An internal error occurred", "detail": m}))
      }
      AppError::PipelineHaltedByHandler => {
        HttpResponse::Conflict().json(json!({"error": "Process halted as expected by business logic."}))
      }
    }
  }
}

// Define a Result type alias for the application
pub type Result<T, E = AppError> = std::result::Result<T, E>;
