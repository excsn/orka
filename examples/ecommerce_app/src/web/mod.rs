// examples/ecommerce_app/src/web/mod.rs

// Declare child modules
pub mod handlers;
pub mod routes;

// Re-export key items if desired.
// For example, to allow main.rs or tests to easily access routing configuration:
pub use routes::configure_app_routes;