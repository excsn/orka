//! Contains the `Pipeline<TData, Err>` struct definition and methods for its
//! construction and structural modification.

use crate::conditional::builder::ConditionalScopeBuilder;
use crate::core::context::{AnyContextDataExtractor, FinishHandler, Handler};
use crate::core::context_data::ContextData;
use crate::core::step::{SkipCondition, StepDef};
use crate::core::trace::{ObserverSlot, PipelineObserver, StepPhase, TraceCollector};
use crate::error::{OrkaError, OrkaResult};
use parking_lot::Mutex;
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

  /// Declared ordering invariants (`before`, `after`), checked by [`Pipeline::validate`].
  pub(crate) ordering_constraints: Vec<(String, String)>,

  /// Declared resource production, as (resource, producing step).
  pub(crate) produced_resources: Vec<(String, String)>,

  /// Declared resource consumption, as (resource, consuming step).
  pub(crate) consumed_resources: Vec<(String, String)>,

  /// Run-level finish handlers, invoked on every exit of a full `run()`.
  pub(crate) finish_handlers: Vec<FinishHandler<TData, Err>>,

  /// The attached execution observer, if any. Interior-mutable (`&self` attachment) and
  /// shared, so conditional master handlers that captured the slot at finalize time still
  /// see a later attachment.
  pub(crate) observer: ObserverSlot,
}

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
        skip_label: None,
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
      ordering_constraints: Vec::new(),
      produced_resources: Vec::new(),
      consumed_resources: Vec::new(),
      finish_handlers: Vec::new(),
      observer: Arc::new(Mutex::new(None)),
    }
  }


  /// Attaches an execution observer. Every subsequent run reports its
  /// [`TraceEvent`](crate::TraceEvent)s to it; at most one observer is attached at a time
  /// (attaching replaces the previous one).
  ///
  /// Takes `&self`: the slot is interior-mutable, so an observer can be attached to a
  /// pipeline already shared behind an `Arc`, for example one obtained from
  /// [`Orka::pipeline`](crate::Orka::pipeline). The observer is snapshotted at run start,
  /// so attaching while a run is in flight misses that run and catches the next one.
  pub fn set_observer(&self, observer: Arc<dyn PipelineObserver>) -> &Self {
    *self.observer.lock() = Some(observer);
    self
  }

  /// Convenience for [`set_observer`](Self::set_observer) with a [`TraceCollector`]: keep
  /// a clone of the collector to read events from, attach the other.
  pub fn set_tracer(&self, tracer: TraceCollector) -> &Self {
    self.set_observer(Arc::new(tracer))
  }

  /// Detaches any attached observer. Subsequent runs record nothing.
  pub fn clear_observer(&self) -> &Self {
    *self.observer.lock() = None;
    self
  }


  /// The pipeline's step names, in execution order.
  pub fn step_names(&self) -> Vec<String> {
    self.steps.iter().map(|s| s.name.clone()).collect()
  }

  /// Whether the step has at least one handler registered for the given phase.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn has_handlers(&self, step_name: impl AsRef<str>, phase: StepPhase) -> bool {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    let map = match phase {
      StepPhase::Before => &self.before,
      StepPhase::On => &self.on,
      StepPhase::After => &self.after,
    };
    map.get(step_name).is_some_and(|v| !v.is_empty())
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
  fn ensure_step_not_exists(&self, step_name: impl AsRef<str>) {
    let step_name = step_name.as_ref();
    if self.steps.iter().any(|s| s.name == step_name) {
      panic!(
        "Orka setup error: Step '{}' already exists in pipeline definition.",
        step_name
      );
    }
  }


  /// Inserts a new required step immediately before an existing one.
  ///
  /// Chain [`optional`](Self::optional) / [`skip_if`](Self::skip_if) afterwards to configure it.
  ///
  /// # Panics
  /// Panics if `existing_step_name` is unknown or `new_step_name` already exists.
  pub fn insert_before_step(&mut self, existing_step_name: impl AsRef<str>, new_step_name: impl AsRef<str>) -> &mut Self {
    let existing_step_name = existing_step_name.as_ref();
    self.ensure_step_exists(existing_step_name); // Fail fast if target doesn't exist
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap(); // Safe due to ensure_step_exists above
    let name_str: String = new_step_name.as_ref().to_string();
    self.ensure_step_not_exists(&name_str); // Prevent duplicate step names
    self.steps.insert(
      idx,
      StepDef {
        name: name_str,
        optional: false,
        skip_if: None,
        skip_label: None,
      },
    );
    self
  }

  /// Inserts a new required step immediately after an existing one.
  ///
  /// # Panics
  /// Panics if `existing_step_name` is unknown or `new_step_name` already exists.
  pub fn insert_after_step(&mut self, existing_step_name: impl AsRef<str>, new_step_name: impl AsRef<str>) -> &mut Self {
    let existing_step_name = existing_step_name.as_ref();
    self.ensure_step_exists(existing_step_name);
    let idx = self.steps.iter().position(|s| s.name == existing_step_name).unwrap();
    let name_str: String = new_step_name.as_ref().to_string();
    self.ensure_step_not_exists(&name_str);
    self.steps.insert(
      idx + 1,
      StepDef {
        name: name_str,
        optional: false,
        skip_if: None,
        skip_label: None,
      },
    );
    self
  }

  /// Removes a step and everything registered against it. Removing an unknown step is a no-op.
  ///
  /// [`must_precede`](Self::must_precede) constraints and
  /// [`produces`](Self::produces)/[`consumed_by`](Self::consumed_by) declarations
  /// referencing the step are deliberately **not** removed: they dangle, and
  /// [`validate`](Self::validate) reports them, so deleting a step other steps declared a
  /// dependency on is caught rather than silently forgotten.
  pub fn remove_step(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    if let Some(idx) = self.steps.iter().position(|s| s.name == step_name) {
      self.steps.remove(idx);
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
  pub fn optional(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self.steps.iter_mut().find(|s| s.name == step_name).unwrap().optional = true;
    self
  }

  /// Marks a step required (the default). Inverse of [`optional`](Self::optional).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn required(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
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
    step_name: impl AsRef<str>,
    cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static,
  ) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    let skip: SkipCondition<TData> = Arc::new(cond);
    let step = self.steps.iter_mut().find(|s| s.name == step_name).unwrap();
    step.skip_if = Some(skip);
    step.skip_label = None; // re-registering unlabeled clears any stale label
    self
  }

  /// As [`skip_if`](Self::skip_if), but with a human-readable label explaining the
  /// condition ("drain disabled by config"). The label is carried into
  /// [`resolve_plan`](Self::resolve_plan) output and `StepSkipped` trace events via
  /// [`SkipReason::SkipCondition`](crate::SkipReason), so previews and skip-matrix test
  /// assertions read as documentation instead of an anonymous skip.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn skip_if_labeled(
    &mut self,
    step_name: impl AsRef<str>,
    label: impl Into<String>,
    cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static,
  ) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    let skip: SkipCondition<TData> = Arc::new(cond);
    let step = self.steps.iter_mut().find(|s| s.name == step_name).unwrap();
    step.skip_if = Some(skip);
    step.skip_label = Some(label.into());
    self
  }

  /// Clears any skip condition (and its label) previously set by
  /// [`skip_if`](Self::skip_if) / [`skip_if_labeled`](Self::skip_if_labeled).
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn clear_skip_condition(&mut self, step_name: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    let step = self.steps.iter_mut().find(|s| s.name == step_name).unwrap();
    step.skip_if = None;
    step.skip_label = None;
    self
  }

  /// Declares an ordering invariant: `before` must appear earlier in the step order than
  /// `after`. Checked by [`validate`](Self::validate), not at run time; declare the
  /// data-threading invariants ("unpack sets `release`, base-labels reads it") once, next
  /// to where the pipeline is assembled, and a bad `insert_before_step`/reorder edit fails
  /// at startup validate instead of panicking mid-run.
  ///
  /// Deliberate semantic: [`remove_step`](Self::remove_step) does **not** clean these
  /// constraints. Removing a step that a constraint references makes `validate` fail with
  /// a dangling-constraint error, which is the point: it catches someone deleting a
  /// producer step whose consumers still exist.
  ///
  /// # Panics
  /// Panics if either step does not exist at declaration time, or if `before == after`.
  pub fn must_precede(&mut self, before: impl AsRef<str>, after: impl AsRef<str>) -> &mut Self {
    let before = before.as_ref();
    let after = after.as_ref();
    self.ensure_step_exists(before);
    self.ensure_step_exists(after);
    if before == after {
      panic!(
        "Orka setup error: must_precede('{}', '{}') names the same step twice.",
        before, after
      );
    }
    self.ordering_constraints.push((before.to_string(), after.to_string()));
    self
  }

  /// [`must_precede`](Self::must_precede) against several later steps at once.
  ///
  /// ```ignore
  /// p.must_precede_all(Step::Unpack, [Step::BaseLabels, Step::LoadSpec, Step::Secrets]);
  /// ```
  ///
  /// When the reason those steps must follow is that they all read something the first
  /// one wrote, prefer [`produces`](Self::produces) / [`consumed_by`](Self::consumed_by):
  /// it says so in the code, and `validate` can then catch a consumer whose producer
  /// disappeared. Reach for this form for orderings that are about effects rather than
  /// data ("drain before stop-unit").
  ///
  /// # Panics
  /// Panics if any step does not exist, or if `before` appears among `afters`.
  pub fn must_precede_all<I, S>(&mut self, before: impl AsRef<str>, afters: I) -> &mut Self
  where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
  {
    let before = before.as_ref().to_string();
    for after in afters {
      self.must_precede(&before, after);
    }
    self
  }


  /// Declares that a step produces a named resource: a value later steps read out of the
  /// context, such as the unpacked release or the parsed app spec.
  ///
  /// Pair with [`consumed_by`](Self::consumed_by). Together they say what a chain of
  /// `must_precede` pairs can only imply, and they let [`validate`](Self::validate) catch
  /// a bug ordering alone cannot express: a step that consumes a resource **nothing
  /// produces**, which today shows up as an `.expect()` panic in the middle of a run after
  /// someone renames or deletes the producing step.
  ///
  /// ```ignore
  /// p.produces(Step::Unpack, Res::Release)
  ///  .consumed_by(Res::Release, [Step::BaseLabels, Step::LoadSpec, Step::Secrets]);
  /// ```
  ///
  /// Resource names, like step names, are any `AsRef<str>`, so a `Res` enum works and
  /// keeps them typo-proof. A resource may have several producers, in which case every
  /// consumer must follow all of them, since which one runs is not known statically.
  ///
  /// # Panics
  /// Panics if the step does not exist.
  pub fn produces(&mut self, step_name: impl AsRef<str>, resource: impl AsRef<str>) -> &mut Self {
    let step_name = step_name.as_ref();
    self.ensure_step_exists(step_name);
    self
      .produced_resources
      .push((resource.as_ref().to_string(), step_name.to_string()));
    self
  }

  /// Declares the steps that read a resource, the counterpart to
  /// [`produces`](Self::produces).
  ///
  /// [`validate`](Self::validate) derives the ordering from this (every producer must
  /// precede every consumer) and reports a consumed resource that no step produces.
  ///
  /// Like [`must_precede`](Self::must_precede), these declarations are **not** cleaned up
  /// by [`remove_step`](Self::remove_step): a dangling one fails `validate`, which is how
  /// deleting a producer that other steps depend on gets caught.
  ///
  /// # Panics
  /// Panics if any step does not exist.
  pub fn consumed_by<I, S>(&mut self, resource: impl AsRef<str>, steps: I) -> &mut Self
  where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
  {
    let resource = resource.as_ref().to_string();
    for step in steps {
      let step = step.as_ref();
      self.ensure_step_exists(step);
      self.consumed_resources.push((resource.clone(), step.to_string()));
    }
    self
  }


  /// Checks the pipeline for setup mistakes that would otherwise surface at run time
  /// (or, worse, silently do nothing).
  ///
  /// Reports:
  /// 1. A required step with no `before`/`on`/`after` handlers — this fails at run time
  ///    with [`OrkaError::HandlerMissing`]; `validate` surfaces it at setup instead.
  /// 2. An extractor registered for a step that has no `on::<SData>` handler using it.
  /// 3. A `conditional_scopes_for_step` builder that was never finalized, so its scopes
  ///    were silently discarded.
  /// 4. A [`must_precede`](Self::must_precede) ordering constraint that is violated by the
  ///    actual step order, or that references a step no longer in the pipeline.
  /// 5. A [`produces`](Self::produces) / [`consumed_by`](Self::consumed_by) declaration
  ///    referencing a step no longer in the pipeline.
  /// 6. A resource that is consumed but that **no step produces**, which is what a renamed
  ///    or deleted producer looks like before it becomes a runtime panic.
  /// 7. A resource consumed by a step that runs earlier than the step producing it.
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

    for (before, after) in &self.ordering_constraints {
      let before_pos = self.steps.iter().position(|s| &s.name == before);
      let after_pos = self.steps.iter().position(|s| &s.name == after);
      match (before_pos, after_pos) {
        (Some(b), Some(a)) => {
          if b >= a {
            problems.push((
              before.clone(),
              format!(
                "ordering constraint violated: '{}' (index {}) must precede '{}' (index {})",
                before, b, after, a
              ),
            ));
          }
        }
        _ => {
          let missing = if before_pos.is_none() { before } else { after };
          problems.push((
            missing.clone(),
            format!(
              "ordering constraint '{}' -> '{}' references unknown step '{}' (was it removed after the constraint was declared?)",
              before, after, missing
            ),
          ));
        }
      }
    }

    // Resource declarations: dangling step references first, then the two things the
    // producer/consumer model can see that bare ordering pairs cannot.
    let step_pos = |name: &str| self.steps.iter().position(|s| s.name == name);

    for (label, declarations) in [
      ("producer", &self.produced_resources),
      ("consumer", &self.consumed_resources),
    ] {
      for (resource, step) in declarations.iter() {
        if step_pos(step).is_none() {
          problems.push((
            step.clone(),
            format!(
              "'{}' is declared as a {} of resource '{}' but is no longer a step in this pipeline",
              step, label, resource
            ),
          ));
        }
      }
    }

    // A resource nothing produces, reported once per resource with all of its consumers,
    // since they share one root cause.
    let mut unproduced: Vec<(String, Vec<String>)> = Vec::new();
    for (resource, consumer) in &self.consumed_resources {
      if step_pos(consumer).is_none() {
        continue;
      }
      let has_producer = self
        .produced_resources
        .iter()
        .any(|(r, p)| r == resource && step_pos(p).is_some());
      if has_producer {
        continue;
      }
      match unproduced.iter_mut().find(|(r, _)| r == resource) {
        Some((_, consumers)) => consumers.push(consumer.clone()),
        None => unproduced.push((resource.clone(), vec![consumer.clone()])),
      }
    }
    for (resource, consumers) in unproduced {
      problems.push((
        consumers[0].clone(),
        format!(
          "resource '{}' is consumed by {} but no step produces it (was the producing step renamed or removed?)",
          resource,
          consumers
            .iter()
            .map(|c| format!("'{}'", c))
            .collect::<Vec<_>>()
            .join(", ")
        ),
      ));
    }

    // Every producer must precede every consumer: which producer runs is not knowable
    // statically, so a consumer sitting between two of them is a real ordering bug.
    for (resource, consumer) in &self.consumed_resources {
      let Some(consumer_idx) = step_pos(consumer) else {
        continue;
      };
      for (produced_resource, producer) in &self.produced_resources {
        if produced_resource != resource {
          continue;
        }
        let Some(producer_idx) = step_pos(producer) else {
          continue;
        };
        if producer_idx >= consumer_idx {
          problems.push((
            producer.clone(),
            format!(
              "resource '{}' is produced by '{}' (index {}) but consumed by '{}' (index {}), which runs earlier",
              resource, producer, producer_idx, consumer, consumer_idx
            ),
          ));
        }
      }
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


  /// Prepares a step to host conditional scoped pipeline executions.
  /// Returns a `ConditionalScopeBuilder<TData, Err>`.
  ///
  /// The step is created if it does not already exist.
  ///
  /// You **must** terminate the returned chain with
  /// [`finalize_conditional_step`](ConditionalScopeBuilder::finalize_conditional_step) —
  /// otherwise the configured scopes are discarded. [`validate`](Self::validate) reports
  /// this if you forget.
  pub fn conditional_scopes_for_step(&mut self, step_name: impl AsRef<str>) -> ConditionalScopeBuilder<'_, TData, Err> {
    let step_name = step_name.as_ref();
    if !self.steps.iter().any(|s| s.name == step_name) {
      self.steps.push(StepDef {
        name: step_name.to_string(),
        optional: false, // Default, can be changed by finalize_conditional_step
        skip_if: None,
        skip_label: None,
      });
    }

    ConditionalScopeBuilder::new(self, step_name.to_string())
  }
}
