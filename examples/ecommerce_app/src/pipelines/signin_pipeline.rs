use crate::errors::AppError;
use crate::models::user::User;
use crate::pipelines::contexts::SigninCtxData;
use crate::services::auth_service;
use crate::state::AppState;
use orka::{Orka, OrkaResult, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::{event, warn, Level};

/// Registers the user sign-in pipeline with the Orka registry.
pub fn register_signin_pipeline(orka_instance: &Arc<Orka<AppError>>, _app_state: &AppState) -> OrkaResult<()> {
  let mut signin_p = Pipeline::<SigninCtxData, AppError>::new([
    "validate_signin_input",
    "fetch_user_by_email_signin",
    "verify_user_password_signin",
    "issue_session_token_signin",
  ]);

  signin_p
    .on_root("validate_signin_input", |ctx_data| async move {
      let (email_val, password_is_empty_val) = {
        let guard = ctx_data.read();
        (guard.email.clone(), guard.password.is_empty())
      };

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
    .on_root("fetch_user_by_email_signin", |ctx_data| async move {
      let (email_val, db_pool_clone) = {
        let guard = ctx_data.read();
        (guard.email.clone(), guard.app_state.db_pool.clone())
      };

      event!(Level::DEBUG, email = %email_val, "Fetching user by email for signin.");

      match sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, created_at, updated_at FROM users WHERE email = $1",
      )
      .bind(&email_val)
      .fetch_optional(&db_pool_clone)
      .await
      {
        Ok(Some(user)) => {
          {
            let mut guard = ctx_data.write();
            guard.user_id = Some(user.id);
            guard.user_email_for_response = Some(user.email.clone());
            guard.temp_password_hash = Some(user.password_hash);
          }
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
    .on_root("verify_user_password_signin", |ctx_data| async move {
      let (stored_hash_opt, password_val, user_id_opt) = {
        let guard = ctx_data.read();
        (guard.temp_password_hash.clone(), guard.password.clone(), guard.user_id)
      };

      let Some(stored_hash) = stored_hash_opt else {
        event!(
          Level::ERROR,
          "Password hash missing in context for verification (signin). Pipeline logic error."
        );
        return Err(AppError::Internal(
          "Password hash unexpectedly missing for verification.".to_string(),
        ));
      };
      event!(Level::DEBUG, user_id = ?user_id_opt, "Verifying password for signin.");

      let verify_result = auth_service::verify_password(&stored_hash, &password_val);
      ctx_data.write().temp_password_hash = None;

      match verify_result {
        Ok(true) => {
          event!(Level::INFO, user_id = ?user_id_opt, "Password verified successfully for signin.");
          Ok(PipelineControl::Continue)
        }
        Ok(false) => {
          warn!("Password mismatch for user_id (signin): {:?}", user_id_opt);
          Err(AppError::Auth("Invalid email or password.".to_string()))
        }
        Err(app_auth_err) => {
          event!(Level::ERROR, error = %app_auth_err, "Error during password verification logic for signin.");
          Err(app_auth_err)
        }
      }
    })
    .on_root("issue_session_token_signin", |ctx_data| async move {
      let user_id_val = { ctx_data.read().user_id.expect("User ID must be present to issue token.") };

      event!(Level::DEBUG, user_id = %user_id_val, "Issuing mock session token.");
      let mock_token = format!("mock_session_token_for_user_{}", user_id_val);

      ctx_data.write().session_token = Some(mock_token.clone());

      event!(Level::INFO, user_id = %user_id_val, token = %mock_token, "Session token issued.");
      Ok(PipelineControl::Continue)
    });

  orka_instance.register_pipeline(signin_p)?;
  tracing::info!("Sign-in pipeline registered.");
  Ok(())
}
