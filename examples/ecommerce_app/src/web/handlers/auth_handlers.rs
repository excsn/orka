use actix_web::{web, HttpResponse};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, instrument, warn};

use crate::errors::AppError;
use crate::pipelines::contexts::{SigninCtxData, SignupCtxData};
use crate::state::AppState;
use orka::{ContextData, PipelineResult};

#[derive(Deserialize, Debug)]
pub struct SignupRequestPayload {
  pub email: String,
  pub password: String,
}

#[derive(Deserialize, Debug)]
pub struct SigninRequestPayload {
  pub email: String,
  pub password: String,
}

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
  let signup_ctx_initial = SignupCtxData {
    app_state: app_state.get_ref().clone(),
    email: req_payload.email.clone(),
    password: req_payload.password.clone(),
    created_user_id: None,
    welcome_email_sent: false,
  };
  let orka_context_data = ContextData::new(signup_ctx_initial);
  // Orka::run returns Result<PipelineResult, AppError>
  match app_state.orka_instance.run(orka_context_data.clone()).await {
    Ok(PipelineResult::Completed) => {
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
      Ok(HttpResponse::Created().json(json!({
          "message": "User created successfully.",
          "userId": user_id.to_string(),
          "email": req_payload.email,
          "welcomeEmailSent": email_sent,
      })))
    }
    Ok(PipelineResult::Stopped) => {
      warn!(
        "Signup pipeline for email {} was stopped by a handler. This might indicate an unhandled business rule.",
        req_payload.email
      );
      Err(AppError::Internal(
        "Signup process was halted by an internal step.".to_string(),
      ))
    }
    Err(app_err) => {
      // AppError implements ResponseError, so Actix renders it directly.
      warn!(
        "Signup pipeline failed for email {}: {:?}",
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
    temp_password_hash: None,
    user_id: None,
    session_token: None,
    user_email_for_response: None,
  };
  let orka_context_data = ContextData::new(signin_ctx_initial);
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
      Ok(HttpResponse::Ok().json(json!({
          "message": "Signin successful.",
          "userId": user_id.to_string(),
          "email": user_email,
          "token": token,
      })))
    }
    Ok(PipelineResult::Stopped) => {
      // A failed sign-in should surface as Err(AppError::Auth(..)); a clean stop is unexpected.
      warn!(
        "Signin pipeline for email {} was stopped by a handler.",
        req_payload.email
      );
      Err(AppError::Auth(
        "Authentication process was unexpectedly halted.".to_string(),
      ))
    }
    Err(app_err) => {
      warn!("Signin pipeline failed for email {}: {:?}", req_payload.email, app_err);
      Err(app_err)
    }
  }
}
