mod common;

use common::*;
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl};

fn config_error_message(err: OrkaError) -> String {
  match err {
    OrkaError::ConfigurationError { message, .. } => message,
    other => panic!("Expected OrkaError::ConfigurationError, got {:?}", other),
  }
}

#[test]
fn test_validate_accepts_a_well_formed_pipeline() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["a", "b"]);
  pipeline
    .optional("b")
    .on_root("a", create_simple_handler("a", "A"));

  assert!(pipeline.validate().is_ok());
}

#[test]
fn test_validate_flags_required_step_without_handlers() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["has_handler", "no_handler"]);
  pipeline.on_root("has_handler", create_simple_handler("has_handler", "H"));

  let message = config_error_message(pipeline.validate().unwrap_err());
  assert!(message.contains("no_handler"), "message was: {}", message);
  assert!(message.contains("no before/on/after handlers"), "message was: {}", message);
}

#[test]
fn test_validate_allows_optional_step_without_handlers() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["a", "spare"]);
  pipeline
    .optional("spare")
    .on_root("a", create_simple_handler("a", "A"));

  assert!(pipeline.validate().is_ok());
}

#[test]
fn test_validate_flags_extractor_with_no_sub_handler() {
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["step"]);
  pipeline
    .set_extractor("step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on_root("step", create_main_extract_context_simple_handler("step", "S"));

  let message = config_error_message(pipeline.validate().unwrap_err());
  assert!(message.contains("extractor"), "message was: {}", message);
  assert!(message.contains("no on::<SData> handler"), "message was: {}", message);
}

#[test]
fn test_validate_accepts_extractor_with_matching_sub_handler() {
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["step"]);
  pipeline
    .set_extractor("step", |main_ctx: ContextData<MainExtractContext>| {
      Ok(main_ctx.project(|d| d.sub_data_container.clone()))
    })
    .on("step", |_s: ContextData<SubExtractContext>| async move {
      Ok(PipelineControl::Continue)
    });

  assert!(pipeline.validate().is_ok());
}

#[test]
fn test_validate_flags_unfinalized_conditional_scope() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["cond"]);

  // Building the scope chain but never calling finalize_conditional_step() silently
  // discards it. `validate` is what catches that.
  let _builder = pipeline.conditional_scopes_for_step("cond");

  let message = config_error_message(pipeline.validate().unwrap_err());
  assert!(message.contains("cond"), "message was: {}", message);
  assert!(
    message.contains("finalize_conditional_step"),
    "message was: {}",
    message
  );
}

#[test]
fn test_validate_accepts_finalized_conditional_scope() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["cond"]);

  pipeline
    .conditional_scopes_for_step("cond")
    .if_no_scope_matches(PipelineControl::Continue)
    .finalize_conditional_step(false);

  assert!(pipeline.validate().is_ok());
}

/// `validate` collects every problem rather than bailing on the first. Note a single step
/// can contribute more than one: `extractor_step` and `unfinalized` are each *also* required
/// steps with no handlers, so this pipeline reports 5 problems across 3 steps.
#[test]
fn test_validate_reports_all_problems_at_once() {
  let mut pipeline = Pipeline::<MainExtractContext, TestError>::new(["missing_handler", "extractor_step"]);
  pipeline.set_extractor("extractor_step", |main_ctx: ContextData<MainExtractContext>| {
    Ok(main_ctx.project(|d| d.sub_data_container.clone()))
  });
  let _builder = pipeline.conditional_scopes_for_step("unfinalized");

  let message = config_error_message(pipeline.validate().unwrap_err());
  assert!(message.contains("5 problems"), "message was: {}", message);
  assert!(message.contains("missing_handler"), "message was: {}", message);
  assert!(message.contains("no on::<SData> handler"), "message was: {}", message);
  assert!(
    message.contains("finalize_conditional_step"),
    "message was: {}",
    message
  );
}

#[tokio::test]
async fn test_register_pipeline_rejects_invalid_pipeline() {
  let registry = Orka::<TestError>::new();
  let pipeline = Pipeline::<TestContext, TestError>::new(["step_with_no_handler"]);

  let result = registry.register_pipeline(pipeline);

  assert!(result.is_err(), "registry must validate on registration");
  let message = config_error_message(result.unwrap_err());
  assert!(message.contains("step_with_no_handler"), "message was: {}", message);
}

#[tokio::test]
async fn test_register_pipeline_accepts_valid_pipeline() {
  let registry = Orka::<TestError>::new();
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["step"]);
  pipeline.on_root("step", create_simple_handler("step", "S"));

  assert!(registry.register_pipeline(pipeline).is_ok());

  let ctx = ContextData::new(TestContext::default());
  assert!(registry.run(ctx.clone()).await.is_ok());
  assert_eq!(ctx.read().steps_executed, vec!["step"]);
}

#[test]
fn test_remove_step_clears_pending_conditional() {
  let mut pipeline = Pipeline::<TestContext, TestError>::new(["a", "cond"]);
  pipeline.on_root("a", create_simple_handler("a", "A"));

  let _builder = pipeline.conditional_scopes_for_step("cond");
  assert!(pipeline.validate().is_err());

  pipeline.remove_step("cond");
  assert!(
    pipeline.validate().is_ok(),
    "removing the step should clear its pending conditional record"
  );
}
