// orka_project/examples/ecommerce_app/src/services/payment_mock.rs
use crate::errors::{AppError, Result as AppResult};
use tracing::{info, instrument};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct MockPaymentIntent {
  pub id: String,
  pub amount: u32,
  pub currency: String,
  pub status: String,                // "requires_action", "succeeded", "failed"
  pub client_secret: Option<String>, // For frontend simulation
  pub account_id_used: String,
}

#[instrument(skip(account_id), fields(order_id, amount, currency, payment_account_id = %account_id))]
pub async fn create_mock_payment_intent(
  order_id: Uuid,
  amount: u32,
  currency: &str,
  account_id: &str, // Simulate using different accounts
) -> AppResult<MockPaymentIntent> {
  info!("Simulating creation of payment intent for account '{}'", account_id);
  // Simulate some basic logic
  if amount == 0 {
    return Err(AppError::Payment("Amount must be greater than zero".to_string()));
  }
  tokio::time::sleep(std::time::Duration::from_millis(50)).await; // Simulate network latency

  let intent_id = format!("mock_pi_{}", Uuid::new_v4());
  Ok(MockPaymentIntent {
    id: intent_id.clone(),
    amount,
    currency: currency.to_string(),
    status: "requires_action".to_string(), // Initial status
    client_secret: Some(format!("{}_secret_{}", intent_id, Uuid::new_v4())),
    account_id_used: account_id.to_string(),
  })
}

#[instrument(skip(intent), fields(payment_intent_id = %intent.id))]
pub async fn confirm_mock_payment(
  intent: &mut MockPaymentIntent, // Takes mutable ref to update status
                                  // payment_method_token: &str, // Simulate a token from frontend
) -> AppResult<()> {
  info!("Simulating confirmation of payment intent ID: {}", intent.id);
  tokio::time::sleep(std::time::Duration::from_millis(100)).await; // Simulate processing

  // Simulate success/failure based on some criteria (e.g., amount, or just always succeed)
  if intent.amount % 1000 == 123 {
    // Arbitrary failure condition
    intent.status = "failed".to_string();
    info!("Mock payment FAILED for intent ID: {}", intent.id);
    Err(AppError::Payment(
      "Mock payment failed due to test condition".to_string(),
    ))
  } else {
    intent.status = "succeeded".to_string();
    info!("Mock payment SUCCEEDED for intent ID: {}", intent.id);
    Ok(())
  }
}
