// examples/ecommerce_app/src/web/handlers/auth_handlers.rs

use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize; // For request payloads
use serde_json::json; // For JSON responses
use tracing::{info, instrument, warn, Level};

use crate::errors::AppError; // Your application specific error
use crate::pipelines::contexts::{SigninCtxData, SignupCtxData};
use crate::state::AppState;
use orka::{ContextData, PipelineResult}; // Orka types

// --- Request DTOs ---
#[derive(Deserialize, Debug)]
pub struct SignupRequestPayload {
  pub email: String,
  pub password: String,
  // pub name: Option<String>, // Optional: if you collect name at signup
}

#[derive(Deserialize, Debug)]
pub struct SigninRequestPayload {
  pub email: String,
  pub password: String,
}

// --- Handler Implementations ---

#[instrument(
    name = "handler::signup",
    skip(app_state, req_payload),
    fields(req_email = %req_payload.email)
)]
pub async fn signup_handler(
  app_state: web::Data<AppState>,
  req_payload: web::Json<SignupRequestPayload>,
) -> Result<HttpResponse, AppError> {
  info!("Signup attempt for email: {}", req_payload.email);

  // 1. Prepare the initial context data for the signup pipeline
  let signup_ctx_initial = SignupCtxData {
    app_state: app_state.get_ref().clone(), // Clone AppState for the context
    email: req_payload.email.clone(),
    password: req_payload.password.clone(),
    // These fields will be populated by the pipeline:
    created_user_id: None,
    welcome_email_sent: false,
  };
  let orka_context_data = ContextData::new(signup_ctx_initial);

  // 2. Run the signup pipeline
  // Orka::run returns Result<PipelineResult, AppError>
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    // Use orka_context_data.clone() because run might be called multiple times in tests or complex scenarios
    // though for a single HTTP request, direct move might also be okay if context isn't needed after.
    Ok(PipelineResult::Completed) => {
      // Pipeline completed successfully.
      // Read results from the (potentially modified) context.
      let final_ctx_guard = orka_context_data.read();
      let user_id = final_ctx_guard.created_user_id.ok_or_else(|| {
        warn!("Signup pipeline completed but user_id was not set in context.");
        AppError::Internal("Signup completed without creating a user ID.".to_string())
      })?;
      let email_sent = final_ctx_guard.welcome_email_sent;

      info!(
        "Signup successful for email: {}. User ID: {}. Welcome email sent: {}",
        req_payload.email, user_id, email_sent
      );

      // 3. Construct and return HTTP response
      Ok(HttpResponse::Created().json(json!({
          "message": "User created successfully.",
          "userId": user_id.to_string(),
          "email": req_payload.email,
          "welcomeEmailSent": email_sent,
      })))
    }
    Ok(PipelineResult::Stopped) => {
      // Pipeline was explicitly stopped by a handler.
      // This might indicate a validation failure handled gracefully within the pipeline,
      // but for signup, completion is usually expected.
      // The specific reason for stopping should ideally be an AppError returned by the pipeline.
      warn!(
        "Signup pipeline for email {} was stopped by a handler. This might indicate an unhandled business rule.",
        req_payload.email
      );
      // Consider if AppError::PipelineHaltedByHandler is appropriate or a more specific error
      // based on data in orka_context_data if the pipeline set any flags.
      Err(AppError::Internal(
        "Signup process was halted by an internal step.".to_string(),
      ))
    }
    Err(app_err) => {
      // Pipeline failed with an AppError. This is the expected path for business errors
      // like "email already exists", validation failures, DB errors, etc.
      // AppError already implements ResponseError, so Actix will handle it.
      warn!(
        "Signup pipeline failed for email {}: {:?}", // Use debug for full AppError
        req_payload.email, app_err
      );
      Err(app_err)
    }
  }
}

#[instrument(
    name = "handler::signin",
    skip(app_state, req_payload),
    fields(req_email = %req_payload.email)
)]
pub async fn signin_handler(
  app_state: web::Data<AppState>,
  req_payload: web::Json<SigninRequestPayload>,
) -> Result<HttpResponse, AppError> {
  info!("Signin attempt for email: {}", req_payload.email);

  // 1. Prepare initial context for the signin pipeline
  let signin_ctx_initial = SigninCtxData {
    app_state: app_state.get_ref().clone(),
    email: req_payload.email.clone(),
    password: req_payload.password.clone(),
    // These fields will be populated by the pipeline:
    temp_password_hash: None,
    user_id: None,
    session_token: None,
    user_email_for_response: None,
  };
  let orka_context_data = ContextData::new(signin_ctx_initial);

  // 2. Run the signin pipeline
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
      let final_ctx_guard = orka_context_data.read();
      let user_id = final_ctx_guard.user_id.ok_or_else(|| {
        warn!("Signin pipeline completed but user_id was not set.");
        AppError::Auth("Signin completed without user identification.".to_string())
      })?;
      let token = final_ctx_guard.session_token.clone().ok_or_else(|| {
        warn!("Signin pipeline completed but session token was not generated.");
        AppError::Auth("Signin completed without session token generation.".to_string())
      })?;
      let user_email = final_ctx_guard.user_email_for_response.clone().unwrap_or_default();

      info!(
        "Signin successful for email: {}. User ID: {}",
        req_payload.email, user_id
      );

      // 3. Construct and return HTTP response
      Ok(HttpResponse::Ok().json(json!({
          "message": "Signin successful.",
          "userId": user_id.to_string(),
          "email": user_email,
          "token": token,
      })))
    }
    Ok(PipelineResult::Stopped) => {
      warn!(
        "Signin pipeline for email {} was stopped by a handler.",
        req_payload.email
      );
      // This usually means authentication failed (e.g. bad password), which should have
      // resulted in an Err(AppError::Auth(...)) from the pipeline itself.
      // If it stops cleanly, it's an unexpected state for a typical signin flow.
      Err(AppError::Auth(
        "Authentication process was unexpectedly halted.".to_string(),
      ))
    }
    Err(app_err) => {
      // Handles AppError::Auth("Invalid email or password"), AppError::Validation, etc.
      warn!("Signin pipeline failed for email {}: {:?}", req_payload.email, app_err);
      Err(app_err)
    }
  }
}
