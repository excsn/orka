//! Defines the structure for a single step within a pipeline.

use super::ContextData;

pub type SkipCondition<TData> = std::sync::Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>;

/// Definition of a pipeline step, including its name, optionality, and skip condition.
///
/// This struct is generic over `T` because the `skip_if` condition operates on the main context `T`.
#[derive(Clone)] // Clone is needed if Vec<StepDef<T>> is cloned (e.g., during pipeline modification or inspection)
pub struct StepDef<T: 'static + Send + Sync> {
  pub name: String,
  pub optional: bool,
  // Condition to evaluate before executing the step. If true, the step is skipped.
  pub skip_if: Option<SkipCondition<T>>,
  // Human-readable label for the skip condition, set by `skip_if_labeled`. Carried into
  // `SkipReason::SkipCondition` in trace events and `resolve_plan` output.
  pub(crate) skip_label: Option<String>,
}

impl<T: 'static + Send + Sync> std::fmt::Debug for StepDef<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("StepDef")
      .field("name", &self.name)
      .field("optional", &self.optional)
      .field("skip_if_present", &self.skip_if.is_some())
      .field("skip_label", &self.skip_label)
      .finish()
  }
}
