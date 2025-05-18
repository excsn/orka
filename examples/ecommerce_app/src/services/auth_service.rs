// orka_project/examples/ecommerce_app/src/services/auth_service.rs

//! Provides authentication-related services like password hashing and verification.

use crate::errors::AppError; // Application-specific error type
use argon2::{
  password_hash::{
    rand_core::OsRng, // For generating random salts
    PasswordHash,
    PasswordHasher,   // The main trait for hashing
    PasswordVerifier, // The main trait for verifying
    SaltString,
  },
  Argon2, // The Argon2 algorithm instance
};
use tracing::{debug, error, instrument};

/// Hashes a plain-text password using Argon2.
///
/// # Arguments
/// * `password`: The plain-text password to hash.
///
/// # Returns
/// A `Result` containing the Argon2 password hash string on success,
/// or an `AppError` if hashing fails or the password is empty.
#[instrument(name = "auth_service::hash_password", skip(password), err(Display))]
pub fn hash_password(password: &str) -> Result<String, AppError> {
  debug!("Attempting to hash password.");
  if password.is_empty() {
    error!("Password hashing failed: Password cannot be empty.");
    return Err(AppError::Validation(
      "Password cannot be empty for hashing.".to_string(),
    ));
  }

  let salt = SaltString::generate(&mut OsRng); // Generate a cryptographically secure random salt
  let argon2_hasher = Argon2::default(); // Use default Argon2 parameters (recommended)

  match argon2_hasher.hash_password(password.as_bytes(), &salt) {
    Ok(password_hash_obj) => {
      debug!("Password hashed successfully.");
      Ok(password_hash_obj.to_string()) // Convert the PasswordHash object to its string representation
    }
    Err(argon_err) => {
      error!(error = %argon_err, "Argon2 password hashing failed.");
      Err(AppError::Internal(format!(
        "Password hashing process failed: {}",
        argon_err
      )))
    }
  }
}

/// Verifies a plain-text password against a stored Argon2 hash.
///
/// # Arguments
/// * `hashed_password_str`: The stored Argon2 password hash string.
/// * `provided_password`: The plain-text password provided by the user for verification.
///
/// # Returns
/// A `Result` containing `true` if the password matches the hash, `false` otherwise.
/// Returns an `AppError` if the hash string is invalid or verification encounters an internal error.
#[instrument(name = "auth_service::verify_password", skip(hashed_password_str, provided_password), err(Display), fields(hash_len = hashed_password_str.len()))]
pub fn verify_password(hashed_password_str: &str, provided_password: &str) -> Result<bool, AppError> {
  debug!("Attempting to verify password.");
  if hashed_password_str.is_empty() {
    error!("Password verification failed: Stored hash string is empty.");
    // This could be considered an auth failure or an internal error depending on context.
    // For sign-in, it would likely lead to an auth failure for the user.
    return Err(AppError::Auth("Invalid stored password format (empty).".to_string()));
  }
  if provided_password.is_empty() {
    error!("Password verification failed: Provided password is empty.");
    return Err(AppError::Auth(
      "Provided password for verification cannot be empty.".to_string(),
    ));
  }

  // Parse the stored hash string into a PasswordHash object
  let parsed_hash = match PasswordHash::new(hashed_password_str) {
    Ok(ph) => ph,
    Err(parse_err) => {
      error!(error = %parse_err, "Failed to parse stored password hash string.");
      // This indicates a problem with the stored hash, likely an internal issue or data corruption.
      return Err(AppError::Internal(format!(
        "Invalid stored password hash format: {}",
        parse_err
      )));
    }
  };

  let argon2_verifier = Argon2::default();

  // Verify the password
  match argon2_verifier.verify_password(provided_password.as_bytes(), &parsed_hash) {
    Ok(()) => {
      debug!("Password verification successful: Passwords match.");
      Ok(true) // Password matches
    }
    Err(argon2::password_hash::Error::Password) => {
      debug!("Password verification failed: Passwords do not match.");
      Ok(false) // Password does not match
    }
    Err(other_argon_err) => {
      error!(error = %other_argon_err, "Argon2 password verification process encountered an error.");
      // Other errors during verification are typically internal.
      Err(AppError::Internal(format!(
        "Password verification process failed: {}",
        other_argon_err
      )))
    }
  }
}

// Placeholder for mock token generation/validation if you were building a full auth system.
// For the current example, pipeline contexts will just hold a mock string token.
/*
#[instrument(name = "auth_service::generate_mock_token", fields(user_id = %user_id))]
pub fn generate_mock_session_token(user_id: uuid::Uuid) -> String {
    let token = format!("mock_token_{}_{}", user_id, uuid::Uuid::new_v4());
    debug!(%token, "Generated mock session token.");
    token
}

#[instrument(name = "auth_service::validate_mock_token", skip(token), err)]
pub fn validate_mock_session_token(token: &str) -> Result<uuid::Uuid, AppError> {
    debug!(%token, "Validating mock session token.");
    if token.starts_with("mock_token_") {
        let parts: Vec<&str> = token.split('_').collect();
        if parts.len() == 3 {
            if let Ok(uid) = uuid::Uuid::parse_str(parts[1]) {
                debug!("Mock token validated successfully for user ID: {}", uid);
                return Ok(uid);
            }
        }
    }
    error!("Mock token validation failed.");
    Err(AppError::Auth("Invalid or expired session token.".to_string()))
}
*/
