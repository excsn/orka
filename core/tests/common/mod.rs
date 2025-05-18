// tests/common/mod.rs
#![allow(dead_code)] // Allow unused code in this common test module

use orka::{ContextData, OrkaError, PipelineControl};
use std::sync::{
  atomic::{AtomicI32, AtomicUsize, Ordering},
  Arc,
};
use tracing::Level;

// --- Common Context Structs ---
#[derive(Clone, Debug, Default)]
pub struct TestContext {
  pub counter: i32,
  pub message: String,
  pub steps_executed: Vec<String>,
  pub should_stop_at: Option<String>,
  pub data_for_scoped: Option<String>, // For conditional tests
  pub scoped_a_ran: bool,              // For conditional tests
  pub scoped_b_ran: bool,              // For conditional tests
}

#[derive(Clone, Debug, Default)]
pub struct ScopedTestContextA {
  pub input: String,
  pub processed_message: String,
}

#[derive(Clone, Debug, Default)]
pub struct ScopedTestContextB {
  pub input: String,
  pub alternative_message: String,
}

// --- Common Error Type for Tests ---
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)] // Clone, PartialEq, Eq for assertions
pub enum TestError {
  #[error("Orka framework error: {0:?}")] // Use :? for OrkaError as it doesn't impl PartialEq
  Orka(String), // Store as String for Eq comparison

  #[error("Test handler failed: {0}")]
  Handler(String),

  #[error("Test extractor failed: {0}")]
  Extractor(String),

  #[error("Test pipeline provider failed: {0}")]
  Provider(String),

  #[error("Test scoped task failed: {0}")]
  ScopedTask(String),
}

impl From<OrkaError> for TestError {
  fn from(oe: OrkaError) -> Self {
    // Simple conversion for testing, might lose some detail but good for Eq.
    // In a real app, you'd preserve more info.
    TestError::Orka(format!("{:?}", oe))
  }
}

// --- Common Handler Creators ---
pub fn create_simple_handler(
  step_name: &'static str,
  message_to_append: &'static str,
) -> orka::Handler<TestContext, TestError> {
  Box::new(move |ctx: ContextData<TestContext>| {
    let step_name_owned = step_name.to_string();
    Box::pin(async move {
      let mut guard = ctx.write();
      guard.counter += 1;
      guard.message.push_str(message_to_append);
      guard.steps_executed.push(step_name_owned.clone());
      tracing::debug!(target: "test_handlers", step = %step_name_owned, "executed, counter: {}, message: '{}'", guard.counter, guard.message);
      if let Some(stop_step) = &guard.should_stop_at {
        if stop_step == step_name_owned.as_str() {
          return Ok(PipelineControl::Stop);
        }
      }
      Ok(PipelineControl::Continue)
    })
  })
}

pub fn create_failing_handler(
  step_name: &'static str,
  error_message: &'static str,
) -> orka::Handler<TestContext, TestError> {
  Box::new(move |ctx: ContextData<TestContext>| {
    let step_name_owned = step_name.to_string();
    let error_message_owned = error_message.to_string();
    Box::pin(async move {
      ctx.write().steps_executed.push(step_name_owned.clone());
      tracing::warn!(target: "test_handlers", step = %step_name_owned, "failing with: '{}'", error_message_owned);
      Err(TestError::Handler(error_message_owned))
    })
  })
}

// --- Helper for Tracing Setup (call once per test run if needed) ---
use once_cell::sync::Lazy;
static TRACING_INIT: Lazy<()> = Lazy::new(|| {
  tracing_subscriber::fmt()
    .with_max_level(Level::DEBUG)
    .with_test_writer() // Important for tests to capture output
    .try_init()
    .ok(); // Allow multiple initializations in tests (ok if fails)
});

pub fn setup_tracing() {
  Lazy::force(&TRACING_INIT);
}

// --- Atomic counters for checking execution counts ---
pub static HANDLER_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static SCOPED_A_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static SCOPED_B_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static EXTRACTOR_A_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static EXTRACTOR_B_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static PROVIDER_A_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));
pub static PROVIDER_B_EXEC_COUNTER: Lazy<Arc<AtomicUsize>> = Lazy::new(|| Arc::new(AtomicUsize::new(0)));

pub fn reset_counters() {
  HANDLER_EXEC_COUNTER.store(0, Ordering::SeqCst);
  SCOPED_A_EXEC_COUNTER.store(0, Ordering::SeqCst);
  SCOPED_B_EXEC_COUNTER.store(0, Ordering::SeqCst);
  EXTRACTOR_A_EXEC_COUNTER.store(0, Ordering::SeqCst);
  EXTRACTOR_B_EXEC_COUNTER.store(0, Ordering::SeqCst);
  PROVIDER_A_EXEC_COUNTER.store(0, Ordering::SeqCst);
  PROVIDER_B_EXEC_COUNTER.store(0, Ordering::SeqCst);
}

// --- Contexts for Sub-Context Extraction Tests ---
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MainExtractContext {
  pub main_field: String,
  pub sub_data_container: SubExtractContext, // SData is a field within TData
  pub counter: i32,
  pub steps_executed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubExtractContext {
  pub sub_field: String,
  pub processed: bool,
}

// Another SData type for mismatch testing
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OtherSubContext {
  pub other_field: i32,
}

// Helper handler for sub-context tests
pub fn create_sub_context_handler(
  step_name: &'static str,
  message_to_append_to_sub_field: &'static str,
) -> Box<
  dyn Fn(
      ContextData<SubExtractContext>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PipelineControl, TestError>> + Send>>
    + Send
    + Sync,
> {
  Box::new(move |sctx: ContextData<SubExtractContext>| {
    let step_name_owned = step_name.to_string();
    Box::pin(async move {
      let mut guard = sctx.write();
      guard.sub_field.push_str(message_to_append_to_sub_field);
      guard.processed = true;
      tracing::debug!(target: "test_handlers_sub", step = %step_name_owned, "sub_handler executed, sub_field: '{}'", guard.sub_field);
      // We need to access the main context's counter to increment it
      // This is tricky if SData is totally separate.
      // For this example, let's assume SubHandler doesn't modify main_ctx.counter directly.
      // The main_ctx modification will be done by a root handler.
      Ok(PipelineControl::Continue)
    })
  })
}

pub fn create_main_extract_context_simple_handler(
  step_name: &'static str,
  message_to_append_to_main_field: &'static str, // Example: operates on main_field
) -> orka::Handler<MainExtractContext, TestError> {
  Box::new(move |ctx: ContextData<MainExtractContext>| {
    let step_name_owned = step_name.to_string();
    Box::pin(async move {
      let mut guard = ctx.write();
      guard.counter += 1; // Assuming MainExtractContext has 'counter'
      guard.main_field.push_str(message_to_append_to_main_field);
      guard.steps_executed.push(step_name_owned.clone());
      tracing::debug!(target: "test_handlers_main_extract", step = %step_name_owned, "executed, counter: {}, main_field: '{}'", guard.counter, guard.main_field);
      Ok(PipelineControl::Continue)
    })
  })
}
