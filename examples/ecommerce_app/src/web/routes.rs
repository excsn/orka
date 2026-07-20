use actix_web::web;

async fn health_check_handler() -> actix_web::HttpResponse {
  actix_web::HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

pub fn configure_app_routes(cfg: &mut web::ServiceConfig) {
  cfg.service(
    web::scope("/api/v1")
      .route("/health", web::get().to(health_check_handler))
      .service(
        web::scope("/auth")
          .route(
            "/signup",
            web::post().to(crate::web::handlers::auth_handlers::signup_handler),
          )
          .route(
            "/signin",
            web::post().to(crate::web::handlers::auth_handlers::signin_handler),
          ), // TODO: add /auth/signout, /auth/me
      )
      .service(
        web::scope("/cart").route(
          "/add",
          web::post().to(crate::web::handlers::cart_handlers::add_to_cart_handler),
        ), // TODO: add GET /cart to view, and a remove-item route
      )
      .service(
        web::scope("/checkout").route(
          "",
          web::post().to(crate::web::handlers::checkout_handlers::start_checkout_handler),
        ), // TODO: add a checkout-status route
      )
      .service(
        web::scope("/webhooks")
          // {source} identifies which provider sent the webhook.
          .route(
            "/{source}",
            web::post().to(crate::web::handlers::webhook_handlers::generic_webhook_handler),
          ),
      ) // TODO: add /orders and admin /users routes
      .service(
        web::scope("/products")
          .route(
            "",
            web::get().to(crate::web::handlers::product_handlers::list_products_handler),
          )
          .route(
            "/{product_id}",
            web::get().to(crate::web::handlers::product_handlers::get_product_handler),
          ),
      ),
  );
}
