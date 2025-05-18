// orka/src/pipeline/definition.rs

//! Contains the `Pipeline<TData, Err>` struct definition and methods for its
//! construction and structural modification.

use crate::conditional::builder::ConditionalScopeBuilder;
use crate::conditional::scope::AnyConditionalScope; // Trait AnyConditionalScope<TData, Err> requires Err: From<OrkaError>
use crate::core::context::{AnyContextDataExtractor, Handler};
use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::core::step::{SkipCondition, StepDef};
use std::collections::HashMap;
// No longer need PhantomData for _phantom_err if Err is constrained on struct
use std::sync::{Arc, Mutex};

/// The core Pipeline type, generic over an underlying root data type `TData`
/// and an error type `Err` that its handlers return.
///
/// `TData` must be `'static + Send + Sync`.
/// `Err` must be `std::error::Error + Send + Sync + 'static` and additionally
/// `From<crate::error::OrkaError>` due to requirements from the conditional execution
/// features (specifically `AnyConditionalScope<TData, Err>`).
pub struct Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<crate::error::OrkaError> + Send + Sync + 'static,
{
  /// Ordered list of step definitions for this pipeline.
  pub(crate) steps: Vec<StepDef<TData>>,

  // Handlers for different phases of each step.
  pub(crate) before: HashMap<String, Vec<Handler<TData, Err>>>,
  pub(crate) on: HashMap<String, Vec<Handler<TData, Err>>>,
  pub(crate) after: HashMap<String, Vec<Handler<TData, Err>>>,

  pub(crate) extractors: HashMap<String, Arc<dyn AnyContextDataExtractor<TData>>>,

  // Configuration for conditional scopes within steps.
  // The type `Arc<dyn AnyConditionalScope<TData, Err>>` requires `Err: From<OrkaError>`.
  pub(crate) conditional_scopes_config:
    Arc<Mutex<HashMap<String, (Arc<Vec<Arc<dyn AnyConditionalScope<TData, Err>>>>, PipelineControl)>>>,
}

// Since the struct Pipeline<TData, Err> now carries the necessary bounds on Err,
// a single impl block is sufficient for all its methods.
impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<crate::error::OrkaError> + Send + Sync + 'static,
{
  /// Creates a new `Pipeline` with an initial set of step definitions.
  pub fn new(step_defs: &[(&str, bool, Option<SkipCondition<TData>>)]) -> Self {
    let steps = step_defs
      .iter()
      .map(|(name, optional, skip_cond_opt)| StepDef {
        name: (*name).to_string(),
        optional: *optional,
        skip_if: skip_cond_opt.clone(),
      })
      .collect();

    Self {
      steps,
      before: HashMap::new(),
      on: HashMap::new(),
      after: HashMap::new(),
      extractors: HashMap::new(),
      conditional_scopes_config: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  /// Ensures that a step with the given name exists in the pipeline. Panics if not found.
  /// This method is typically used internally before operating on a step.
  pub(crate) fn ensure_step_exists(&self, step_name: &str) {
    if !self.steps.iter().any(|s| s.name == step_name) {
      // This panic is a programming error indicator (e.g., typo in step name).
      // It's not an OrkaError because it's usually a setup issue.
      panic!(
        "Orka setup error: Step '{}' not found in pipeline definition.",
        step_name
      );
    }
  }

  /// Ensures that a step with the given name does NOT exist. Panics if it exists.
  /// Used internally before adding a new step.
  fn ensure_step_not_exists(&self, step_name: &str) {
    if self.steps.iter().any(|s| s.name == step_name) {
      panic!(
        "Orka setup error: Step '{}' already exists in pipeline definition.",
        step_name
      );
    }
  }

  // --- Basic Step Manipulation Methods ---

  pub fn insert_before_step<S: Into<String>>(
    &mut self,
    existing_step_name: &str,
    new_step_name: S,
    optional: bool,
    skip_if: Option<SkipCondition<TData>>,
  ) {
    self.ensure_step_exists(existing_step_name); // Fail fast if target doesn't exist
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap(); // Safe due to ensure_step_exists above
    let name_str: String = new_step_name.into();
    self.ensure_step_not_exists(&name_str); // Prevent duplicate step names
    self.steps.insert(
      idx,
      StepDef {
        name: name_str,
        optional,
        skip_if,
      },
    );
  }

  pub fn insert_after_step<S: Into<String>>(
    &mut self,
    existing_step_name: &str,
    new_step_name: S,
    optional: bool,
    skip_if: Option<SkipCondition<TData>>,
  ) {
    self.ensure_step_exists(existing_step_name);
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap();
    let name_str: String = new_step_name.into();
    self.ensure_step_not_exists(&name_str);
    self.steps.insert(
      idx + 1,
      StepDef {
        name: name_str,
        optional,
        skip_if,
      },
    );
  }

  pub fn remove_step(&mut self, step_name: &str) {
    if let Some(idx) = self.steps.iter().position(|s| s.name == step_name) {
      self.steps.remove(idx);
      // Also remove associated handlers and configurations
      self.before.remove(step_name);
      self.on.remove(step_name);
      self.after.remove(step_name);
      self.extractors.remove(step_name);
      self.conditional_scopes_config.lock().unwrap().remove(step_name);
    } else {
      // Optionally log a warning or do nothing if step to remove isn't found
      // For now, consistent with ensure_step_exists, removal of non-existent step is a no-op.
      // If strictness is required, call ensure_step_exists first or panic here.
    }
  }

  pub fn set_optional(&mut self, step_name: &str, optional: bool) {
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().optional = optional;
  }

  pub fn set_skip_condition(&mut self, step_name: &str, skip_if: Option<SkipCondition<TData>>) {
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().skip_if = skip_if;
  }

  // --- Entry Point for Conditional Scoped Pipelines ---

  /// Prepares a step to host conditional scoped pipeline executions.
  /// Returns a `ConditionalScopeBuilder<TData, Err>`.
  ///
  /// The pipeline's `Err` type (already constrained on `Pipeline` struct) must be `From<OrkaError>`
  /// for the builder to correctly map framework errors (e.g., from extractors) into `Err`.
  pub fn conditional_scopes_for_step(&mut self, step_name: &str) -> ConditionalScopeBuilder<TData, Err> {
    // Ensure the step definition exists or create it.
    // ConditionalScopeBuilder::new also checks this, but it's good practice here too.
    if !self.steps.iter().any(|s| s.name == step_name) {
      self.steps.push(StepDef {
        name: step_name.to_string(),
        optional: false, // Default, can be changed by finalize_conditional_step
        skip_if: None,
      });
    }

    // Ensure an entry for this step exists in the conditional_scopes_config map.
    // The type of value stored (involving AnyConditionalScope<TData, Err>) is valid
    // because Err on the Pipeline struct already has the From<OrkaError> bound.
    self
      .conditional_scopes_config
      .lock()
      .unwrap()
      .entry(step_name.to_string())
      .or_insert_with(|| (Arc::new(Vec::new()), PipelineControl::Continue));

    ConditionalScopeBuilder::new(self, step_name.to_string())
  }
}
