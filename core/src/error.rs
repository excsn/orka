// orka_core/src/error.rs
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
impl From<AnyhowError> for OrkaError {
  fn from(err: AnyhowError) -> Self {
    // Check if the anyhow::Error is already wrapping an OrkaError
    // to avoid OrkaError(HandlerError(OrkaError(...)))
    if let Some(orka_err) = err.downcast_ref::<OrkaError>() {
        // This requires OrkaError to be Clone, or to reconstruct.
        // For now, let's assume we want to avoid direct cloning if not necessary
        // and just re-wrap for simplicity, or handle specific variants.
        // A simple re-wrap:
        // return OrkaError::HandlerError { source: err }; // This might be fine.

        // If OrkaError is not Clone, we can't just return *orka_err.
        // A common pattern is to stringify and wrap in Internal, or just re-wrap as HandlerError.
        // Given HandlerError takes anyhow::Error, simply returning it is often best.
        return OrkaError::HandlerError { source: err };
    }
    OrkaError::HandlerError { source: err }
  }
}

pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;