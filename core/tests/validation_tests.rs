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

// --- must_precede ordering constraints ---

fn noop(name: &'static str) -> orka::Handler<TestContext, TestError> {
  create_simple_handler(name, "")
}

#[test]
fn must_precede_satisfied_passes_validate() {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "base_labels", "start"]);
  p.on_root("unpack", noop("unpack"))
    .on_root("base_labels", noop("base_labels"))
    .on_root("start", noop("start"))
    .must_precede("unpack", "base_labels")
    .must_precede("base_labels", "start");
  assert!(p.validate().is_ok());
}

#[test]
fn must_precede_violation_is_reported_with_indexes() {
  let mut p = Pipeline::<TestContext, TestError>::new(["base_labels", "unpack"]);
  p.on_root("unpack", noop("unpack"))
    .on_root("base_labels", noop("base_labels"))
    .must_precede("unpack", "base_labels");
  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("ordering constraint violated"), "message was: {}", message);
  assert!(message.contains("'unpack'") && message.contains("'base_labels'"), "message was: {}", message);
}

#[test]
fn must_precede_catches_a_bad_insert() {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "base_labels"]);
  p.on_root("unpack", noop("unpack"))
    .on_root("base_labels", noop("base_labels"))
    .must_precede("unpack", "base_labels");
  assert!(p.validate().is_ok());

  // Someone later inserts a step and, in a follow-up edit, moves the consumer ahead of
  // its producer by re-adding it before "unpack". Simulate with remove + insert_before.
  p.remove_step("base_labels");
  p.insert_before_step("unpack", "base_labels");
  p.on_root("base_labels", noop("base_labels"));
  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("ordering constraint violated"), "message was: {}", message);
}

#[test]
fn must_precede_dangles_loudly_when_a_step_is_removed() {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "base_labels"]);
  p.on_root("unpack", noop("unpack"))
    .on_root("base_labels", noop("base_labels"))
    .must_precede("unpack", "base_labels");

  // remove_step deliberately does NOT clean ordering constraints: the dangle is the signal.
  p.remove_step("unpack");
  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("references unknown step 'unpack'"), "message was: {}", message);
}

#[test]
#[should_panic(expected = "not found in pipeline definition")]
fn must_precede_panics_on_unknown_step() {
  let mut p = Pipeline::<TestContext, TestError>::new(["only"]);
  p.must_precede("only", "missing");
}

#[test]
#[should_panic(expected = "names the same step twice")]
fn must_precede_panics_on_self_reference() {
  let mut p = Pipeline::<TestContext, TestError>::new(["only"]);
  p.must_precede("only", "only");
}

// --- must_precede_all ---

#[test]
fn must_precede_all_expands_to_one_constraint_per_target() {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "labels", "spec", "secrets"]);
  for name in ["unpack", "labels", "spec", "secrets"] {
    p.on_root(name, noop(name));
  }
  p.must_precede_all("unpack", ["labels", "spec", "secrets"]);
  assert!(p.validate().is_ok());

  // Move one consumer ahead of the producer: that pair, and only that pair, fails.
  p.remove_step("spec");
  p.insert_before_step("unpack", "spec");
  p.on_root("spec", noop("spec"));
  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("'unpack'") && message.contains("'spec'"), "message was: {}", message);
  assert!(!message.contains("'labels'"), "unrelated pairs stay quiet: {}", message);
}

// --- produces / consumed_by ---

fn release_pipeline() -> Pipeline<TestContext, TestError> {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "base_labels", "load_spec", "install"]);
  for name in ["unpack", "base_labels", "load_spec", "install"] {
    p.on_root(name, noop(name));
  }
  p.produces("unpack", "release")
    .consumed_by("release", ["base_labels", "load_spec"]);
  p
}

#[test]
fn resource_declarations_pass_when_producer_precedes_consumers() {
  let p = release_pipeline();
  assert!(p.validate().is_ok());
}

#[test]
fn a_consumer_running_before_its_producer_is_reported_with_both_indexes() {
  let mut p = release_pipeline();
  p.remove_step("base_labels");
  p.insert_before_step("unpack", "base_labels");
  p.on_root("base_labels", noop("base_labels"));

  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("resource 'release'"), "message was: {}", message);
  assert!(message.contains("produced by 'unpack'"), "message was: {}", message);
  assert!(message.contains("consumed by 'base_labels'"), "message was: {}", message);
}

/// The bug class ordering pairs cannot express: the producer is renamed, so the resource
/// has no producer at all. Today that is an `.expect()` panic mid-run.
#[test]
fn a_consumed_resource_with_no_producer_is_reported_once_with_all_consumers() {
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack_v2", "base_labels", "load_spec"]);
  for name in ["unpack_v2", "base_labels", "load_spec"] {
    p.on_root(name, noop(name));
  }
  // The producer was renamed to "unpack_v2" but the declaration still names "unpack",
  // which is no longer registered as producing anything.
  p.consumed_by("release", ["base_labels", "load_spec"]);

  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("no step produces it"), "message was: {}", message);
  assert!(
    message.contains("'base_labels'") && message.contains("'load_spec'"),
    "one message names every affected consumer: {}",
    message
  );
  assert_eq!(
    message.matches("no step produces it").count(),
    1,
    "reported once per resource, not once per consumer: {}",
    message
  );
}

#[test]
fn removing_a_producer_step_dangles_its_declaration() {
  let mut p = release_pipeline();
  p.remove_step("unpack");

  let message = config_error_message(p.validate().unwrap_err());
  assert!(
    message.contains("declared as a producer of resource 'release'"),
    "message was: {}",
    message
  );
  // And the consumers are now unproduced, which is the same edit seen from the other side.
  assert!(message.contains("no step produces it"), "message was: {}", message);
}

#[test]
fn every_producer_must_precede_every_consumer() {
  // Two producers of one resource: a consumer between them is a real bug, since which
  // producer runs is not knowable statically.
  let mut p = Pipeline::<TestContext, TestError>::new(["unpack", "consume", "restore_cached"]);
  for name in ["unpack", "consume", "restore_cached"] {
    p.on_root(name, noop(name));
  }
  p.produces("unpack", "release")
    .produces("restore_cached", "release")
    .consumed_by("release", ["consume"]);

  let message = config_error_message(p.validate().unwrap_err());
  assert!(message.contains("produced by 'restore_cached'"), "message was: {}", message);
  assert!(!message.contains("produced by 'unpack'"), "the earlier producer is fine: {}", message);
}

#[test]
#[should_panic(expected = "not found in pipeline definition")]
fn produces_panics_on_unknown_step() {
  let mut p = Pipeline::<TestContext, TestError>::new(["only"]);
  p.produces("missing", "release");
}

#[test]
#[should_panic(expected = "not found in pipeline definition")]
fn consumed_by_panics_on_unknown_step() {
  let mut p = Pipeline::<TestContext, TestError>::new(["only"]);
  p.consumed_by("release", ["only", "missing"]);
}
