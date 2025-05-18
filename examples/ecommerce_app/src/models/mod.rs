// orka_project/examples/ecommerce_app/src/models/mod.rs

//! Contains data structures representing database entities.

// Declare child modules for each model
pub mod cart_item; // Changed from cart.rs to cart_item.rs for clarity if it's about items
pub mod order;
pub mod order_item;
pub mod product;
pub mod user;

// Re-export the model structs for convenient access
pub use cart_item::CartItem;
pub use order::{Order, OrderStatus}; // Assuming OrderStatus enum is in order.rs
pub use order_item::OrderItem;
pub use product::Product;
pub use user::User;
