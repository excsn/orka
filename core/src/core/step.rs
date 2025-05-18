// orka/src/core/step.rs

//! Defines the structure for a single step within a pipeline.

use super::ContextData;

// Type alias for the skip condition closure.
// It takes a read-only reference to the main context T.
// Uses Arc to be easily cloneable and shareable.
pub type SkipCondition<TData> = std::sync::Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>;

/// Definition of a pipeline step, including its name, optionality, and skip condition.
///
/// This struct is generic over `T` because the `skip_if` condition operates on the main context `T`.
#[derive(Clone)] // Clone is needed if Vec<StepDef<T>> is cloned (e.g., during pipeline modification or inspection)
pub struct StepDef<T: 'static + Send + Sync> {
  pub name: String,
  pub optional: bool,
  // Condition to evaluate before executing the step. If true, the step is skipped.
  // Operates on the root context `T`.
  pub skip_if: Option<SkipCondition<T>>,
}

// Manual implementation of Debug if needed, especially if SkipCondition makes it complex.
// By default, SkipCondition (Arc<dyn Fn...>) doesn't implement Debug.
// We can provide a placeholder debug output.
impl<T: 'static + Send + Sync> std::fmt::Debug for StepDef<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("StepDef")
      .field("name", &self.name)
      .field("optional", &self.optional)
      .field("skip_if_present", &self.skip_if.is_some())
      .finish()
  }
}
