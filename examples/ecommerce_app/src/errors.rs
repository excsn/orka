use actix_web::{HttpResponse, ResponseError};
use serde_json::json;
use thiserror::Error;

use orka::OrkaError;

// Stripe/Brevo/PipelineHaltedByHandler are part of the error taxonomy this example
// demonstrates; the mocked services never construct them.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum AppError {
  #[error("Validation Error: {0}")]
  Validation(String),

  #[error("Authentication Failed: {0}")]
  Auth(String),

  #[error("Resource Not Found: {0}")]
  NotFound(String),

  #[error("Payment Processing Error: {0}")]
  Payment(String),

  #[error("Configuration Error: {0}")]
  Config(String),

  #[error("Database Error: {0}")]
  Sqlx(#[from] sqlx::Error),

  #[error("Stripe API Error: {0}")]
  Stripe(String),

  #[error("Brevo Email Error: {0}")]
  Brevo(String),

  #[error("Orka Workflow Error: {source}")]
  Workflow {
    #[from]
    source: OrkaError,
  },

  #[error("Internal Server Error: {0}")]
  Internal(String),

  /// For an HTTP handler that treats a graceful pipeline stop as a request failure.
  #[error("Pipeline execution was halted by a handler.")]
  PipelineHaltedByHandler,
}

/// Lets handlers use `?` on `anyhow::Result`, unwrapping the common inner types.
impl From<anyhow::Error> for AppError {
  fn from(err: anyhow::Error) -> Self {
    // AppError is not Clone, so a wrapped one is re-stated rather than returned as-is.
    if let Some(app_err) = err.downcast_ref::<AppError>() {
      return AppError::Internal(format!("Downcasted AppError: {}", app_err));
    }
    if err.is::<sqlx::Error>() {
      return AppError::Sqlx(err.downcast::<sqlx::Error>().unwrap());
    }

    AppError::Internal(err.to_string())
  }
}

impl ResponseError for AppError {
  fn error_response(&self) -> HttpResponse {
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

pub type Result<T, E = AppError> = std::result::Result<T, E>;
