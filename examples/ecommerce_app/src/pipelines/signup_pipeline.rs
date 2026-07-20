use crate::errors::AppError;
use crate::models::user::User;
use crate::pipelines::common_steps;
use crate::pipelines::contexts::{SendWelcomeEmailCtxData, SignupCtxData};
use crate::services::auth_service;
use crate::state::AppState;
use orka::{ContextData, Orka, OrkaResult, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::{event, info, warn, Level};

/// Registers the user sign-up pipeline with the Orka registry.
pub fn register_signup_pipeline(orka_instance: &Arc<Orka<AppError>>, _app_state: &AppState) -> OrkaResult<()> {
  let mut signup_p = Pipeline::<SignupCtxData, AppError>::new([
    "validate_signup_input",
    "check_existing_user_signup",
    "create_user_in_db",
    "send_welcome_email_signup",
  ]);
  signup_p.optional("send_welcome_email_signup");

  signup_p
    .on_root("validate_signup_input", |ctx_data| async move {
      let (email_val, password_len_val) = {
        let guard = ctx_data.read();
        (guard.email.clone(), guard.password.len())
      };

      event!(Level::DEBUG, email = %email_val, "Validating signup input.");
      if email_val.is_empty() || !email_val.contains('@') {
        warn!("Invalid email format provided for signup.");
        return Err(AppError::Validation("Valid email is required.".to_string()));
      }
      if password_len_val < 8 {
        warn!("Password too short for signup ({} chars).", password_len_val);
        return Err(AppError::Validation(
          "Password must be at least 8 characters long.".to_string(),
        ));
      }
      Ok(PipelineControl::Continue)
    })
    .on_root("check_existing_user_signup", |ctx_data| async move {
      let (email_val, db_pool_clone) = {
        let guard = ctx_data.read();
        (guard.email.clone(), guard.app_state.db_pool.clone())
      };

      event!(Level::DEBUG, email = %email_val, "Checking if user email already exists.");

      match sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
        .bind(&email_val)
        .fetch_one(&db_pool_clone)
        .await
      {
        Ok(true) => {
          warn!("Attempt to signup with existing email: {}", email_val);
          Err(AppError::Validation(
            "An account with this email already exists.".to_string(),
          ))
        }
        Ok(false) => {
          info!("Email {} is available for signup.", email_val);
          Ok(PipelineControl::Continue)
        }
        Err(sqlx_error) => {
          event!(Level::ERROR, error = %sqlx_error, "Database error while checking for existing user.");
          Err(AppError::Sqlx(sqlx_error))
        }
      }
    })
    .on_root("create_user_in_db", |ctx_data| async move {
      let (email_val, password_val, db_pool_clone) = {
        let guard = ctx_data.read();
        (
          guard.email.clone(),
          guard.password.clone(),
          guard.app_state.db_pool.clone(),
        )
      };

      event!(Level::DEBUG, email = %email_val, "Creating user in database.");

      let hashed_password = auth_service::hash_password(&password_val).inspect_err(|app_err| {
        event!(Level::ERROR, error = %app_err, "Password hashing failed during user creation.");
      })?;

      match sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash) VALUES ($1, $2) RETURNING id, email, password_hash, created_at, updated_at",
      )
      .bind(&email_val)
      .bind(hashed_password)
      .fetch_one(&db_pool_clone)
      .await
      {
        Ok(new_user) => {
          ctx_data.write().created_user_id = Some(new_user.id);
          info!("User created successfully: ID={}, Email={}", new_user.id, new_user.email);
          Ok(PipelineControl::Continue)
        }
        Err(sqlx_error) => {
          event!(Level::ERROR, error = %sqlx_error, "Database error while creating user.");
          Err(AppError::Sqlx(sqlx_error))
        }
      }
    })
    .on_root("send_welcome_email_signup", |ctx_data| async move {
      let (app_state_clone, email_val, created_user_id_opt) = {
        let guard = ctx_data.read();
        (guard.app_state.clone(), guard.email.clone(), guard.created_user_id)
      };

      if created_user_id_opt.is_none() {
        warn!(
          "Cannot send welcome email, user ID not set in signup context for email: {}",
          email_val
        );
        return Ok(PipelineControl::Continue);
      }

      let recipient_name = email_val.split('@').next().unwrap_or("User").to_string();
      let email_ctx_data_wrapper = ContextData::new(SendWelcomeEmailCtxData {
        app_state: app_state_clone,
        recipient_email: email_val.clone(),
        recipient_name,
      });

      event!(Level::DEBUG, email = %email_val, "Preparing to send welcome email.");

      match common_steps::send_welcome_email_step(email_ctx_data_wrapper).await {
        Ok(PipelineControl::Continue) => {
          ctx_data.write().welcome_email_sent = true;
          info!("Welcome email step indicated success for {}", email_val);
          Ok(PipelineControl::Continue)
        }
        Ok(PipelineControl::Stop) => {
          warn!("Welcome email step unexpectedly signaled Stop for {}.", email_val);
          ctx_data.write().welcome_email_sent = false;
          Ok(PipelineControl::Stop)
        }
        Err(orka_err) => {
          warn!("Welcome email step failed for {}: {:?}", email_val, orka_err);
          ctx_data.write().welcome_email_sent = false;
          // This step is optional, so a failed email must not fail the signup.
          Ok(PipelineControl::Continue)
        }
      }
    });

  orka_instance.register_pipeline(signup_p)?;
  tracing::info!("Sign-up pipeline registered.");
  Ok(())
}
