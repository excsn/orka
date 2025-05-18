// orka_project/examples/ecommerce_app/src/pipelines/signin_pipeline.rs

use crate::errors::{AppError, Result as AppResult}; // AppError and AppResult
use crate::models::user::User;
use crate::pipelines::contexts::SigninCtxData; // Use *CtxData
use crate::services::auth_service;
use crate::state::AppState;
use anyhow::Error as AnyhowError;
use orka::{ContextData, Orka, OrkaError, OrkaResult, Pipeline, PipelineControl}; // Import Orka types
use std::sync::Arc;
use tracing::{event, instrument, warn, Level}; // For error conversion if needed

/// Registers the user sign-in pipeline with the Orka registry.
pub fn register_signin_pipeline(
  orka_instance: &Arc<Orka<AppError>>,
  _app_state: &AppState, // Not directly used here if AppState is in CtxData
) {
  // Pipeline is Pipeline<SigninCtxData, AppError>
  let mut signin_p = Pipeline::<SigninCtxData, AppError>::new(&[
    ("validate_signin_input", false, None),
    ("fetch_user_by_email_signin", false, None),  // Renamed for clarity
    ("verify_user_password_signin", false, None), // Renamed for clarity
    ("issue_session_token_signin", false, None),
  ]);

  // Step 1: Validate input
  // Handler takes ContextData<SigninCtxData> and returns AppResult<PipelineControl>
  signin_p.on_root("validate_signin_input", |ctx_data: ContextData<SigninCtxData>| {
    Box::pin(async move {
      // Returns AppResult<PipelineControl>
      let (email_val, password_is_empty_val) = {
        // Read scope
        let guard = ctx_data.read();
        (guard.email.clone(), guard.password.is_empty())
      }; // guard dropped

      event!(Level::DEBUG, email = %email_val, "Validating sign-in input.");
      if email_val.is_empty() || !email_val.contains('@') {
        warn!("Invalid email format provided for sign-in.");
        return Err(AppError::Validation("Valid email is required.".to_string()));
      }
      if password_is_empty_val {
        warn!("Empty password provided for sign-in.");
        return Err(AppError::Validation("Password is required.".to_string()));
      }
      Ok(PipelineControl::Continue)
    })
  });

  // Step 2: Fetch user by email
  signin_p.on_root("fetch_user_by_email_signin", |ctx_data: ContextData<SigninCtxData>| {
    Box::pin(async move {
      // Returns AppResult<PipelineControl>
      let (email_val, db_pool_clone) = {
        // Read scope
        let guard = ctx_data.read();
        (guard.email.clone(), guard.app_state.db_pool.clone())
      }; // guard dropped

      event!(Level::DEBUG, email = %email_val, "Fetching user by email for signin.");

      // DB call is await, lock is already dropped
      match sqlx::query_as::<_, User>(
        // Fetch only necessary fields for User struct if not all are needed.
        // Assuming User struct can be created from this subset.
        "SELECT id, email, password_hash, created_at, updated_at FROM users WHERE email = $1",
      )
      .bind(&email_val)
      .fetch_optional(&db_pool_clone)
      .await
      {
        Ok(Some(user)) => {
          {
            // Write scope
            let mut guard = ctx_data.write();
            guard.user_id = Some(user.id);
            guard.user_email_for_response = Some(user.email.clone());
            guard.temp_password_hash = Some(user.password_hash); // Store hash for next step
          } // guard dropped
          event!(Level::INFO, user_id = %user.id, "User found for signin.");
          Ok(PipelineControl::Continue)
        }
        Ok(None) => {
          warn!("User not found for email during signin: {}", email_val);
          Err(AppError::Auth("Invalid email or password.".to_string()))
        }
        Err(sqlx_error) => {
          event!(Level::ERROR, error = %sqlx_error, "Database error while fetching user for signin.");
          Err(AppError::Sqlx(sqlx_error))
        }
      }
    })
  });

  // Step 3: Verify password
  signin_p.on_root("verify_user_password_signin", |ctx_data: ContextData<SigninCtxData>| {
    Box::pin(async move {
      // Returns AppResult<PipelineControl>
      let (stored_hash_opt, password_val, user_id_opt) = {
        // Read scope
        let guard = ctx_data.read();
        (
          guard.temp_password_hash.clone(),
          guard.password.clone(),
          guard.user_id, // For logging
        )
      }; // guard dropped

      let stored_hash = match stored_hash_opt {
        Some(hash) => hash,
        None => {
          event!(
            Level::ERROR,
            "Password hash missing in context for verification (signin). Pipeline logic error."
          );
          return Err(AppError::Internal(
            "Password hash unexpectedly missing for verification.".to_string(),
          ));
        }
      };
      event!(Level::DEBUG, user_id = ?user_id_opt, "Verifying password for signin.");

      // auth_service::verify_password returns Result<bool, AppError>
      match auth_service::verify_password(&stored_hash, &password_val) {
        Ok(true) => {
          event!(Level::INFO, user_id = ?user_id_opt, "Password verified successfully for signin.");
          {
            // Write scope to clear temp hash
            ctx_data.write().temp_password_hash = None;
          } // guard dropped
          Ok(PipelineControl::Continue)
        }
        Ok(false) => {
          warn!("Password mismatch for user_id (signin): {:?}", user_id_opt);
          {
            ctx_data.write().temp_password_hash = None;
          }
          Err(AppError::Auth("Invalid email or password.".to_string()))
        }
        Err(app_auth_err) => {
          // app_auth_err is AppError
          event!(Level::ERROR, error = %app_auth_err, "Error during password verification logic for signin.");
          {
            ctx_data.write().temp_password_hash = None;
          }
          Err(app_auth_err)
        }
      }
    })
  });

  // Step 4: Issue session token (mocked)
  signin_p.on_root("issue_session_token_signin", |ctx_data: ContextData<SigninCtxData>| {
    Box::pin(async move {
      // Returns AppResult<PipelineControl>
      let user_id_val = {
        // Read scope
        let guard = ctx_data.read();
        guard.user_id.expect("User ID must be present to issue token.") // Panics if None, as expected
      }; // guard dropped

      event!(Level::DEBUG, user_id = %user_id_val, "Issuing mock session token.");
      let mock_token = format!("mock_session_token_for_user_{}", user_id_val);

      {
        // Write scope
        let mut guard = ctx_data.write();
        guard.session_token = Some(mock_token.clone()); // Store the generated token
      } // guard dropped

      event!(Level::INFO, user_id = %user_id_val, token = %mock_token, "Session token issued.");
      Ok::<_, AppError>(PipelineControl::Continue)
    })
  });

  orka_instance.register_pipeline(signin_p);
  tracing::info!("Sign-in pipeline registered.");
}
