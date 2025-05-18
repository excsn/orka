// orka_project/examples/ecommerce_app/src/pipelines/signup_pipeline.rs

use crate::errors::{AppError, Result as AppResult}; // AppError and AppResult
use crate::models::user::User;
use crate::pipelines::common_steps;
use crate::pipelines::contexts::{SendWelcomeEmailCtxData, SignupCtxData}; // Use *CtxData
use crate::services::auth_service;
use crate::state::AppState;
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl, OrkaResult}; // Import Orka types
use std::sync::Arc;
use tracing::{event, info, instrument, warn, Level};
use uuid::Uuid;
use anyhow::Error as AnyhowError; // For converting AppError to anyhow for OrkaError::HandlerError

/// Registers the user sign-up pipeline with the Orka registry.
pub fn register_signup_pipeline(
  orka_instance: &Arc<Orka<AppError>>,
  _app_state: &AppState,
) {
  // Pipeline is Pipeline<SignupCtxData, AppError>
  let mut signup_p = Pipeline::<SignupCtxData, AppError>::new(&[
    ("validate_signup_input", false, None),
    ("check_existing_user_signup", false, None),
    // "hash_user_password_signup" is removed as hashing is part of create_user_in_db
    ("create_user_in_db", false, None),
    ("send_welcome_email_signup", true, None), // Optional
  ]);

  // Step 1: Validate input
  // Handler takes ContextData<SignupCtxData> and returns AppResult<PipelineControl>
  signup_p.on_root(
    "validate_signup_input",
    |ctx_data: ContextData<SignupCtxData>| {
      Box::pin(async move { // This block returns AppResult<PipelineControl>
        let (email_val, password_len_val) = { // Read scope
          let guard = ctx_data.read();
          (guard.email.clone(), guard.password.len())
        }; // guard dropped

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
    },
  );

  // Step 2: Check if user with this email already exists
  signup_p.on_root(
    "check_existing_user_signup",
    |ctx_data: ContextData<SignupCtxData>| {
      Box::pin(async move { // Returns AppResult<PipelineControl>
        let (email_val, db_pool_clone) = { // Read scope
          let guard = ctx_data.read();
          (guard.email.clone(), guard.app_state.db_pool.clone())
        }; // guard dropped

        event!(Level::DEBUG, email = %email_val, "Checking if user email already exists.");
        
        // DB call is await, lock is already dropped
        match sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
          .bind(&email_val)
          .fetch_one(&db_pool_clone)
          .await
        {
          Ok(true) => {
            warn!("Attempt to signup with existing email: {}", email_val);
            Err(AppError::Validation("An account with this email already exists.".to_string()))
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
    },
  );

  // Step 3: Create user in the database (includes password hashing)
  signup_p.on_root(
    "create_user_in_db",
    |ctx_data: ContextData<SignupCtxData>| {
      Box::pin(async move { // Returns AppResult<PipelineControl>
        let (email_val, password_val, db_pool_clone) = { // Read scope
          let guard = ctx_data.read();
          (
            guard.email.clone(),
            guard.password.clone(), // Clone password for hashing
            guard.app_state.db_pool.clone(),
          )
        }; // guard dropped

        event!(Level::DEBUG, email = %email_val, "Creating user in database.");
        
        let hashed_password = match auth_service::hash_password(&password_val) {
          Ok(h) => h,
          Err(app_err) => { // auth_service returns Result<_, AppError>
            event!(Level::ERROR, error = %app_err, "Password hashing failed during user creation.");
            return Err(app_err);
          }
        };

        // DB call is await
        match sqlx::query_as::<_, User>(
              "INSERT INTO users (email, password_hash) VALUES ($1, $2) RETURNING id, email, password_hash, created_at, updated_at",
          )
          .bind(&email_val)
          .bind(hashed_password)
          .fetch_one(&db_pool_clone)
          .await
          {
              Ok(new_user) => {
                  { // Write scope
                    let mut guard = ctx_data.write();
                    guard.created_user_id = Some(new_user.id);
                  } // guard dropped
                  info!("User created successfully: ID={}, Email={}", new_user.id, new_user.email);
                  Ok(PipelineControl::Continue)
              }
              Err(sqlx_error) => {
                  event!(Level::ERROR, error = %sqlx_error, "Database error while creating user.");
                  Err(AppError::Sqlx(sqlx_error))
              }
          }
      })
    },
  );

  // Step 4: Send welcome email (optional step, uses common_steps)
  signup_p.on_root(
    "send_welcome_email_signup",
    |ctx_data: ContextData<SignupCtxData>| {
      Box::pin(async move { // Returns AppResult<PipelineControl>
        let (app_state_clone, email_val, created_user_id_opt) = { // Read scope
            let guard = ctx_data.read();
            (guard.app_state.clone(), guard.email.clone(), guard.created_user_id)
        }; // guard dropped

        if created_user_id_opt.is_none() {
            warn!("Cannot send welcome email, user ID not set in signup context for email: {}", email_val);
            // This step is optional, so we continue even if a prerequisite for sending is missing.
            // Alternatively, this could be an internal error if user_id *should* always be set here.
            return Ok::<_, AppError>(PipelineControl::Continue);
        }

        let recipient_name = email_val.split('@').next().unwrap_or("User").to_string();
        
        // Prepare ContextData for the common step
        let email_ctx_underlying = SendWelcomeEmailCtxData {
            app_state: app_state_clone,
            recipient_email: email_val.clone(),
            recipient_name,
        };
        let email_ctx_data_wrapper = ContextData::new(email_ctx_underlying);

        event!(Level::DEBUG, email = %email_val, "Preparing to send welcome email.");
        
        // common_steps::send_welcome_email_step returns OrkaResult<PipelineControl>
        // This handler needs to return AppResult. So, map the error.
        match common_steps::send_welcome_email_step(email_ctx_data_wrapper).await {
            Ok(PipelineControl::Continue) => {
                { // Write scope
                    ctx_data.write().welcome_email_sent = true;
                } // guard dropped
                info!("Welcome email step indicated success for {}", email_val);
                Ok(PipelineControl::Continue)
            }
            Ok(PipelineControl::Stop) => { // Should not happen from current common_step logic
                warn!("Welcome email step unexpectedly signaled Stop for {}.", email_val);
                { ctx_data.write().welcome_email_sent = false; }
                Ok(PipelineControl::Stop) // Propagate if it happens
            }
            Err(orka_err) => { // orka_err is OrkaError
                warn!("Welcome email step failed for {}: {:?}", email_val, orka_err);
                { ctx_data.write().welcome_email_sent = false; }
                // This pipeline step is optional, so we continue the main pipeline.
                // The error from the common_step is logged by tracing.
                // We don't convert it to AppError and return Err(...) because that would halt
                // a non-optional pipeline if this step wasn't marked optional.
                Ok(PipelineControl::Continue)
            }
        }
      })
    },
  );

  orka_instance.register_pipeline(signup_p);
  tracing::info!("Sign-up pipeline registered.");
}