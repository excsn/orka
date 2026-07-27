// tests/common/mod.rs
#![allow(dead_code)]

use orka::{ContextData, OrkaError, Pipeline, PipelineControl};
use std::sync::Arc;
use tracing::Level;

/// The shared test error, shipped by orka itself (feature `test-util`) so downstream
/// crates get the same one. `Clone + PartialEq`, stringifies `OrkaError` via `From`.
pub use orka::test_util::TestError;

/// What a scoped-pipeline factory resolves to. Factories in the conditional tests are
/// synchronous, so they return `Ready` rather than a boxed future.
pub type ReadyScopedPipeline<SData> = std::future::Ready<Result<Arc<Pipeline<SData, TestError>>, OrkaError>>;

/// A boxed sub-context handler, as produced by [`create_sub_context_handler`].
pub type BoxedSubHandler<SData> = Box<
  dyn Fn(ContextData<SData>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PipelineControl, TestError>> + Send>>
    + Send
    + Sync,
>;

#[derive(Clone, Debug, Default)]
pub struct TestContext {
  pub counter: i32,
  pub message: String,
  pub steps_executed: Vec<String>,
  pub should_stop_at: Option<String>,
  pub data_for_scoped: Option<String>,
  pub scoped_a_ran: bool,
  pub scoped_b_ran: bool,
  /// Filled by a conditional scope's `with_merge`, so tests can assert on what a scope
  /// produced without reaching for global counters.
  pub scoped_result: Option<String>,
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
      if let Some(stop_step) = &guard.should_stop_at
        && stop_step == step_name_owned.as_str()
      {
        return Ok(PipelineControl::Stop);
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

use once_cell::sync::Lazy;
static TRACING_INIT: Lazy<()> = Lazy::new(|| {
  tracing_subscriber::fmt()
    .with_max_level(Level::DEBUG)
    .with_test_writer()
    .try_init()
    .ok();
});

pub fn setup_tracing() {
  Lazy::force(&TRACING_INIT);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MainExtractContext {
  pub main_field: String,
  pub sub_data_container: SubExtractContext,
  pub counter: i32,
  pub steps_executed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubExtractContext {
  pub sub_field: String,
  pub processed: bool,
}

/// A second `SData` type, used to exercise the downcast type-mismatch path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OtherSubContext {
  pub other_field: i32,
}

pub fn create_sub_context_handler(
  step_name: &'static str,
  message_to_append_to_sub_field: &'static str,
) -> BoxedSubHandler<SubExtractContext> {
  Box::new(move |sctx: ContextData<SubExtractContext>| {
    let step_name_owned = step_name.to_string();
    Box::pin(async move {
      let mut guard = sctx.write();
      guard.sub_field.push_str(message_to_append_to_sub_field);
      guard.processed = true;
      tracing::debug!(target: "test_handlers_sub", step = %step_name_owned, "sub_handler executed, sub_field: '{}'", guard.sub_field);
      Ok(PipelineControl::Continue)
    })
  })
}

pub fn create_main_extract_context_simple_handler(
  step_name: &'static str,
  message_to_append_to_main_field: &'static str,
) -> orka::Handler<MainExtractContext, TestError> {
  Box::new(move |ctx: ContextData<MainExtractContext>| {
    let step_name_owned = step_name.to_string();
    Box::pin(async move {
      let mut guard = ctx.write();
      guard.counter += 1;
      guard.main_field.push_str(message_to_append_to_main_field);
      guard.steps_executed.push(step_name_owned.clone());
      tracing::debug!(target: "test_handlers_main_extract", step = %step_name_owned, "executed, counter: {}, main_field: '{}'", guard.counter, guard.main_field);
      Ok(PipelineControl::Continue)
    })
  })
}
