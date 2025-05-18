// orka_project/examples/ecommerce_app/src/pipelines/mod.rs

//! Defines and registers all Orka pipelines used by the e-commerce application.

use crate::errors::AppError; // The application-specific error type
use crate::state::AppState;
use std::sync::Arc;
use orka::Orka; // The Orka registry

// Declare sub-modules for different parts of pipeline definitions
pub mod contexts;     // Defines all T and S context structs
pub mod factories;    // Defines factory functions for dynamic Scoped Pipelines (e.g., mock payment providers)
pub mod common_steps; // (Optional) For reusable individual pipeline steps/handlers

// Declare modules for each major workflow/pipeline
pub mod signup_pipeline;
pub mod signin_pipeline;
pub mod cart_pipeline; // Could group AddToCart, ViewCart, RemoveFromCart logic
pub mod checkout_pipeline;
pub mod webhook_pipeline; // For generic webhook processing if needed

// Add more pipeline modules as your application grows (e.g., product_catalog_pipeline, user_profile_pipeline)

/// Registers all defined Orka pipelines with the provided Orka registry instance.
///
/// This function is typically called once at application startup.
pub fn register_all_pipelines(
    orka_instance: &Arc<Orka<AppError>>,
    app_state: &AppState, // Pass AppState if pipeline definitions or factories need it
) {
    tracing::info!("Registering Orka pipelines...");

    // Call registration functions from each pipeline module
    signup_pipeline::register_signup_pipeline(orka_instance, app_state);
    signin_pipeline::register_signin_pipeline(orka_instance, app_state);
    checkout_pipeline::register_checkout_pipeline(orka_instance, app_state);
    cart_pipeline::register_add_to_cart_pipeline(orka_instance, app_state);
    webhook_pipeline::register_webhook_pipeline(orka_instance, app_state); 

    tracing::info!("All application pipelines registered with Orka.");
}