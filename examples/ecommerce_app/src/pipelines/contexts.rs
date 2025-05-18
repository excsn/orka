// orka_project/examples/ecommerce_app/src/pipelines/contexts.rs

//! Defines all underlying data structs used by Orka pipelines.
//! Handlers will receive these wrapped in `orka::ContextData`.

use crate::config::AppConfig;
use crate::models;
use crate::services::payment_mock::MockPaymentIntent;
use crate::state::AppState;
use orka::ContextData;
use std::sync::Arc;
use uuid::Uuid; // Crucial import

// --- Main Pipeline Underlying Data Structs (TData) ---

#[derive(Clone)]
pub struct SignupCtxData {
  pub app_state: AppState,
  pub email: String,
  pub password: String,
  pub created_user_id: Option<Uuid>,
  pub welcome_email_sent: bool,
}

#[derive(Clone)]
pub struct SigninCtxData {
  pub app_state: AppState,
  pub email: String,
  pub password: String,
  pub temp_password_hash: Option<String>,
  pub user_id: Option<Uuid>,
  pub session_token: Option<String>,
  pub user_email_for_response: Option<String>,
}

#[derive(Clone)]
pub struct AddToCartCtxData {
  pub app_state: AppState,
  pub authenticated_user_id: Uuid,
  pub product_id: Uuid,
  pub quantity: i32,
  pub updated_cart_item: Option<models::cart_item::CartItem>,
}

// --- Checkout Process Underlying Data Structs ---

/// Underlying data for a mock payment provider's sub-pipeline (SData).
#[derive(Debug, Clone)]
pub struct MockPaymentProviderSubCtxData {
  pub order_id: Uuid,
  pub amount_cents: u32, // Note: was u32, payment_mock takes u32
  pub currency: String,
  pub using_account_id: String,
  pub payment_intent: Option<MockPaymentIntent>,
  pub succeeded: bool,
}

/// Enum to hold the active payment provider's sub-context.
/// Each variant now directly holds `ContextData<MockPaymentProviderSubCtxData>`
/// allowing the sub-pipeline to operate on its own lockable context.
#[derive(Debug, Clone)]
pub enum ActivePaymentDetails {
  None,
  MockProviderA(ContextData<MockPaymentProviderSubCtxData>),
  MockProviderB(ContextData<MockPaymentProviderSubCtxData>),
}

/// Underlying data for the checkout orchestrating pipeline (TData).
#[derive(Clone)]
pub struct CheckoutCtxData {
  pub app_state: AppState,
  pub authenticated_user_id: Uuid,
  pub order_id: Option<Uuid>,
  pub cart_items_value_cents: u32, // Note: was u32 in MockPaymentProviderSubCtxData
  pub currency_code: String,
  pub chosen_payment_method: String,
  // This field stores the account ID string that was used to *initialize* the sub-context
  // when `determine_payment_route_and_init_sub_context_checkout` runs.
  pub current_payment_account_id_for_sub_ctx_init: Option<String>, // Renamed from payment_account_id_for_tx
  pub payment_sub_context: ActivePaymentDetails,                   // Holds ContextData<SData>
  pub payment_processing_overall_success: bool,
  pub order_finalized_in_db: bool,
  pub confirmation_email_sent: bool,
  // Added for the confirmation email example
  pub user_email_for_confirmation: Option<String>,
  pub user_name_for_confirmation: Option<String>,
}

// --- Other Underlying Data Structs (for common steps, etc.) ---

#[derive(Clone)]
pub struct SendWelcomeEmailCtxData {
  pub app_state: AppState,
  pub recipient_email: String,
  pub recipient_name: String,
  // Field to update if needed by the caller
  // pub email_message_id: Option<String>,
}

#[derive(Clone)]
pub struct SendOrderConfirmationEmailCtxData {
  pub app_state: AppState,
  pub recipient_email: String,
  pub recipient_name: String,
  pub order_id: Uuid,
  pub order_total_display: String,
  // pub email_message_id: Option<String>,
}

#[derive(Clone)]
pub struct GenericWebhookCtxData {
  pub app_state: AppState,
  pub raw_payload: actix_web::web::Bytes,
  pub source_identifier: String,
  pub signature_header: Option<String>,
  pub event_processed: bool,
  pub affected_order_id: Option<Uuid>,
}
