//! Contains the `Pipeline<TData, Err>` struct definition and methods for its
//! construction and structural modification.

use crate::conditional::builder::ConditionalScopeBuilder;
use crate::core::context::{AnyContextDataExtractor, Handler};
use crate::core::context_data::ContextData;
use crate::core::step::{SkipCondition, StepDef};
use crate::error::{OrkaError, OrkaResult};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

  /// Steps whose `conditional_scopes_for_step` builder was created but never finalized.
  /// Surfaced by [`Pipeline::validate`]; a step left in here has no conditional behaviour.
  pub(crate) pending_conditional: HashSet<String>,

  /// Steps that have at least one `on::<SData>` sub-handler registered. Used by
  /// [`Pipeline::validate`] to flag extractors that nothing consumes.
  pub(crate) sub_handler_steps: HashSet<String>,
}

// Since the struct Pipeline<TData, Err> now carries the necessary bounds on Err,
// a single impl block is sufficient for all its methods.
impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<crate::error::OrkaError> + Send + Sync + 'static,
{
  /// Creates a new `Pipeline` from an ordered list of step names.
  ///
  /// Every step starts out **required** with no skip condition. Use the chainable
  /// [`optional`](Self::optional) and [`skip_if`](Self::skip_if) setters to adjust:
  ///
  /// ```ignore
  /// let mut pipeline = Pipeline::<Ctx, MyError>::new(["load", "validate", "notify"]);
  /// pipeline
  ///   .optional("notify")
  ///   .skip_if("validate", |ctx| ctx.read().already_valid);
  /// ```
  ///
  /// Accepts anything iterable of string-likes — `&["a", "b"]`, `["a", "b"]`, `Vec<String>`.
  ///
  /// # Panics
  /// Panics if the same step name appears twice.
  pub fn new<I, S>(step_names: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
  {
    let mut steps: Vec<StepDef<TData>> = Vec::new();

    for name in step_names {
      let name = name.as_ref().to_string();
      if steps.iter().any(|s: &StepDef<TData>| s.name == name) {
        panic!("Orka setup error: duplicate step '{}' in pipeline definition.", name);
      }
      steps.push(StepDef {
        name,
        optional: false,
        skip_if: None,
      });
    }

    Self {
      steps,
      before: HashMap::new(),
      on: HashMap::new(),
      after: HashMap::new(),
      extractors: HashMap::new(),
      pending_conditional: HashSet::new(),
      sub_handler_steps: HashSet::new(),
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

  /// Inserts a new required step immediately before an existing one.
  ///
  /// Chain [`optional`](Self::optional) / [`skip_if`](Self::skip_if) afterwards to configure it.
  ///
  /// # Panics
  /// Panics if `existing_step_name` is unknown or `new_step_name` already exists.
  pub fn insert_before_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S) -> &mut Self {
    self.ensure_step_exists(existing_step_name); // Fail fast if target doesn't exist
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap(); // Safe due to ensure_step_exists above
    let name_str: String = new_step_name.into();
    self.ensure_step_not_exists(&name_str); // Prevent duplicate step names
    self.steps.insert(
      idx,
      StepDef {
        name: name_str,
        optional: false,
        skip_if: None,
      },
    );
    self
  }

  /// Inserts a new required step immediately after an existing one.
  ///
  /// # Panics
  /// Panics if `existing_step_name` is unknown or `new_step_name` already exists.
  pub fn insert_after_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S) -> &mut Self {
    self.ensure_step_exists(existing_step_name);
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap();
    let name_str: String = new_step_name.into();
    self.ensure_step_not_exists(&name_str);
    self.steps.insert(
      idx + 1,
      StepDef {
        name: name_str,
        optional: false,
        skip_if: None,
      },
    );
    self
  }

  /// Removes a step and everything registered against it. Removing an unknown step is a no-op.
  pub fn remove_step(&mut self, step_name: &str) -> &mut Self {
    if let Some(idx) = self.steps.iter().position(|s| s.name == step_name) {
      self.steps.remove(idx);
      // Also remove associated handlers and configurations
      self.before.remove(step_name);
      self.on.remove(step_name);
      self.after.remove(step_name);
      self.extractors.remove(step_name);
      self.pending_conditional.remove(step_name);
      self.sub_handler_steps.remove(step_name);
    }
    self
  }

  /// Marks a step optional: it is skipped rather than failing when it has no handlers,
  /// and errors from its conditional scopes are swallowed.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn optional(&mut self, step_name: &str) -> &mut Self {
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().optional = true;
    self
  }

  /// Marks a step required (the default). Inverse of [`optional`](Self::optional).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn required(&mut self, step_name: &str) -> &mut Self {
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().optional = false;
    self
  }

  /// Sets a predicate that, when it returns `true` at run time, skips this step entirely
  /// (no before/on/after handlers run).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn skip_if(
    &mut self,
    step_name: &str,
    cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static,
  ) -> &mut Self {
    self.ensure_step_exists(step_name);
    let skip: SkipCondition<TData> = Arc::new(cond);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().skip_if = Some(skip);
    self
  }

  /// Clears any skip condition previously set by [`skip_if`](Self::skip_if).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn clear_skip_condition(&mut self, step_name: &str) -> &mut Self {
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().skip_if = None;
    self
  }

  // --- Validation ---

  /// Checks the pipeline for setup mistakes that would otherwise surface at run time
  /// (or, worse, silently do nothing).
  ///
  /// Reports:
  /// 1. A required step with no `before`/`on`/`after` handlers — this fails at run time
  ///    with [`OrkaError::HandlerMissing`]; `validate` surfaces it at setup instead.
  /// 2. An extractor registered for a step that has no `on::<SData>` handler using it.
  /// 3. A `conditional_scopes_for_step` builder that was never finalized, so its scopes
  ///    were silently discarded.
  ///
  /// All problems are collected, not just the first. Calling this is optional;
  /// [`Orka::register_pipeline`](crate::Orka::register_pipeline) runs it for you.
  pub fn validate(&self) -> OrkaResult<()> {
    let mut problems: Vec<(String, String)> = Vec::new();

    for step in &self.steps {
      let name = step.name.as_str();
      let has_handlers = [&self.before, &self.on, &self.after]
        .iter()
        .any(|m| m.get(name).is_some_and(|v| !v.is_empty()));

      if !step.optional && !has_handlers {
        problems.push((
          step.name.clone(),
          format!(
            "required step '{}' has no before/on/after handlers; register one or mark it optional",
            name
          ),
        ));
      }
    }

    for step_name in self.extractors.keys() {
      if !self.sub_handler_steps.contains(step_name) {
        problems.push((
          step_name.clone(),
          format!(
            "step '{}' has an extractor but no on::<SData> handler consuming it",
            step_name
          ),
        ));
      }
    }

    for step_name in &self.pending_conditional {
      problems.push((
        step_name.clone(),
        format!(
          "step '{}' called conditional_scopes_for_step but never finalize_conditional_step(); its scopes were discarded",
          step_name
        ),
      ));
    }

    match problems.len() {
      0 => Ok(()),
      1 => {
        let (step_name, message) = problems.pop().unwrap();
        Err(OrkaError::ConfigurationError { step_name, message })
      }
      n => {
        let message = problems
          .iter()
          .map(|(_, m)| format!("  - {}", m))
          .collect::<Vec<_>>()
          .join("\n");
        Err(OrkaError::ConfigurationError {
          step_name: format!("<{} steps>", n),
          message: format!("pipeline validation found {} problems:\n{}", n, message),
        })
      }
    }
  }

  // --- Entry Point for Conditional Scoped Pipelines ---

  /// Prepares a step to host conditional scoped pipeline executions.
  /// Returns a `ConditionalScopeBuilder<TData, Err>`.
  ///
  /// The step is created if it does not already exist.
  ///
  /// You **must** terminate the returned chain with
  /// [`finalize_conditional_step`](ConditionalScopeBuilder::finalize_conditional_step) —
  /// otherwise the configured scopes are discarded. [`validate`](Self::validate) reports
  /// this if you forget.
  pub fn conditional_scopes_for_step(&mut self, step_name: &str) -> ConditionalScopeBuilder<'_, TData, Err> {
    // Ensure the step definition exists or create it.
    // ConditionalScopeBuilder::new also checks this, but it's good practice here too.
    if !self.steps.iter().any(|s| s.name == step_name) {
      self.steps.push(StepDef {
        name: step_name.to_string(),
        optional: false, // Default, can be changed by finalize_conditional_step
        skip_if: None,
      });
    }

    ConditionalScopeBuilder::new(self, step_name.to_string())
  }
}
