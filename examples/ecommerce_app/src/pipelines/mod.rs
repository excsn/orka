//! Defines and registers all Orka pipelines used by the e-commerce application.

use crate::errors::AppError;
use crate::state::AppState;
use orka::{Orka, OrkaResult};
use std::sync::Arc;

pub mod common_steps;
pub mod contexts;
pub mod factories;

pub mod cart_pipeline;
pub mod checkout_pipeline;
pub mod signin_pipeline;
pub mod signup_pipeline;
pub mod webhook_pipeline;

/// Registers all defined Orka pipelines with the provided Orka registry instance.
///
/// This function is typically called once at application startup.
pub fn register_all_pipelines(orka_instance: &Arc<Orka<AppError>>, app_state: &AppState) -> OrkaResult<()> {
  tracing::info!("Registering Orka pipelines...");

  signup_pipeline::register_signup_pipeline(orka_instance, app_state)?;
  signin_pipeline::register_signin_pipeline(orka_instance, app_state)?;
  checkout_pipeline::register_checkout_pipeline(orka_instance, app_state)?;
  cart_pipeline::register_add_to_cart_pipeline(orka_instance, app_state)?;
  webhook_pipeline::register_webhook_pipeline(orka_instance, app_state)?;

  tracing::info!("All application pipelines registered with Orka.");
  Ok(())
}
