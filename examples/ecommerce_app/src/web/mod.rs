// The HTTP layer is defined but deliberately NOT mounted in `main.rs`; the server binds a
// port and serves nothing. Everything below is therefore unreachable, hence the allow.
// To actually serve these routes, add `.configure(web::configure_app_routes)` to the
// `App::new()` chain in main.rs.
#![allow(dead_code)]

pub mod handlers;
pub mod routes;

pub use routes::configure_app_routes;
