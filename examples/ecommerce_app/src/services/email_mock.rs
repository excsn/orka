// orka_project/examples/ecommerce_app/src/services/email_mock.rs
use crate::errors::Result as AppResult; // Using AppResult
use tracing::info;

#[derive(Debug)]
pub struct SentEmailInfo {
  pub to: String,
  pub from: String,
  pub subject: String,
  pub body_preview: String, // First N chars of body
  pub message_id: String,
}

pub async fn send_mock_email(to: &str, from: &str, subject: &str, html_body: &str) -> AppResult<SentEmailInfo> {
  info!(
    "Simulating sending email: To='{}', From='{}', Subject='{}'",
    to, from, subject
  );
  tokio::time::sleep(std::time::Duration::from_millis(20)).await; // Simulate network latency

  // Simulate potential failure
  if subject.to_lowercase().contains("fail_test") {
    tracing::warn!("Simulated email failure for subject: {}", subject);
    return Err(crate::errors::AppError::Internal(
      "Simulated email send failure".to_string(),
    ));
  }

  let body_preview = html_body.chars().take(50).collect::<String>() + "...";
  let message_id = format!("mock_email_{}", uuid::Uuid::new_v4());
  info!("Mock email sent successfully. Message ID: {}", message_id);

  Ok(SentEmailInfo {
    to: to.to_string(),
    from: from.to_string(),
    subject: subject.to_string(),
    body_preview,
    message_id,
  })
}
