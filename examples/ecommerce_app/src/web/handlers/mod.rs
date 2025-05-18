// examples/ecommerce_app/src/web/handlers/mod.rs

// Declare handler modules
pub mod auth_handlers;
pub mod cart_handlers;
pub mod checkout_handlers;
pub mod product_handlers;
pub mod webhook_handlers; // If you have specific handlers for webhooks

// Re-export specific handler functions if you want a flatter access path,
// or let routes.rs access them via their module path (e.g., auth_handlers::signup_handler).
// Example:
// pub use auth_handlers::{signup_handler, signin_handler};
// pub use cart_handlers::add_to_cart_handler;
// pub use checkout_handlers::start_checkout_handler;