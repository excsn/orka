//! Defines all underlying data structs used by Orka pipelines.
//! Handlers will receive these wrapped in `orka::ContextData`.

use crate::models;
use crate::services::payment_mock::MockPaymentIntent;
use crate::state::AppState;
use uuid::Uuid;

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
  #[allow(dead_code)] // The handle a real DB-backed step would read from.
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
  pub amount_cents: u32,
  pub currency: String,
  pub using_account_id: String,
  pub payment_intent: Option<MockPaymentIntent>,
  pub succeeded: bool,
}

/// Underlying data for the checkout orchestrating pipeline (TData).
#[derive(Clone)]
pub struct CheckoutCtxData {
  pub app_state: AppState,
  pub authenticated_user_id: Uuid,
  pub order_id: Option<Uuid>,
  pub cart_items_value_cents: u32,
  pub currency_code: String,
  pub chosen_payment_method: String,
  /// Account ID the chosen provider's scoped pipeline should transact with.
  pub current_payment_account_id_for_sub_ctx_init: Option<String>,
  /// Filled in by the payment scope's `with_merge` once its scoped pipeline finishes.
  pub payment_result: Option<MockPaymentProviderSubCtxData>,
  pub payment_processing_overall_success: bool,
  pub order_finalized_in_db: bool,
  pub confirmation_email_sent: bool,
  pub user_email_for_confirmation: Option<String>,
  pub user_name_for_confirmation: Option<String>,
}

// --- Other Underlying Data Structs (for common steps, etc.) ---

#[derive(Clone)]
pub struct SendWelcomeEmailCtxData {
  #[allow(dead_code)] // Carried so a real sender can reach the pool/config.
  pub app_state: AppState,
  pub recipient_email: String,
  pub recipient_name: String,
}

#[derive(Clone)]
pub struct SendOrderConfirmationEmailCtxData {
  #[allow(dead_code)] // Carried so a real sender can reach the pool/config.
  pub app_state: AppState,
  pub recipient_email: String,
  pub recipient_name: String,
  pub order_id: Uuid,
  pub order_total_display: String,
}

#[derive(Clone)]
pub struct GenericWebhookCtxData {
  #[allow(dead_code)] // The handle a real DB-backed step would read from.
  pub app_state: AppState,
  pub raw_payload: actix_web::web::Bytes,
  pub source_identifier: String,
  pub signature_header: Option<String>,
  pub event_processed: bool,
  pub affected_order_id: Option<Uuid>,
}
