use anyhow::Error as AnyhowError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrkaError {
    #[error("Step not found: {step_name}")]
    StepNotFound { step_name: String },

    #[error("Handler missing for non-optional step: {step_name}")]
    HandlerMissing { step_name: String },

    #[error("Extractor failed for step '{step_name}'. Source: {source}")]
    ExtractorFailure {
        step_name: String,
        #[source]
        source: AnyhowError,
    },

    #[error("Pipeline provider failed for conditional scope in step '{step_name}'. Source: {source}")]
    PipelineProviderFailure {
        step_name: String,
        #[source]
        source: AnyhowError,
    },

    #[error("Type mismatch during context downcast (expected {expected_type}, step: '{step_name}')")]
    TypeMismatch {
        step_name: String,
        expected_type: String,
    },

    #[error("Error in user-provided handler or external operation. Source: {source}")]
    HandlerError {
        #[source]
        source: AnyhowError,
    },
    
    #[error("Configuration error for step '{step_name}': {message}")]
    ConfigurationError { step_name: String, message: String },

    #[error("Internal Orka error: {0}")]
    Internal(String),
    // Add NoConditionalScopeMatched if it's used by the builder
    #[error("No conditional scope's condition matched for step '{step_name}'")]
    NoConditionalScopeMatched { step_name: String },
}

// This is the key conversion Orka provides for external errors.
//
// An `anyhow::Error` that already wraps an `OrkaError` is deliberately left nested rather
// than unwrapped: `OrkaError` is not `Clone`, and `HandlerError` keeps the original as a
// `#[source]`, so the causal chain stays intact either way.
impl From<AnyhowError> for OrkaError {
  fn from(err: AnyhowError) -> Self {
    OrkaError::HandlerError { source: err }
  }
}

pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;