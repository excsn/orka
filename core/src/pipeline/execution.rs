//! Contains `Pipeline::run()` and its variants, responsible for executing the pipeline's
//! steps and handlers, plus the [`resolve_plan`](Pipeline::resolve_plan) dry-run.
//!
//! The pipeline is `Pipeline<TData, Err>`, and `run` returns `Result<PipelineResult, Err>`.

use crate::core::context_data::ContextData;
use crate::core::control::{PipelineControl, PipelineResult};
use crate::core::step::StepDef;
use crate::core::trace::{
  combine_observers, current_scoped_observer, next_run_id, HandlerOutcome, HandlerScope, PipelineObserver, RunOutcome,
  SharedObserver, SkipReason, StepPhase, TraceEvent, TraceEventKind,
};

/// The run id, the observer every event for a run goes to, and the scoped observer to make
/// ambient so nested runs inherit it.
type RunObservation = (u64, Option<SharedObserver>, Option<SharedObserver>);
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline;
use std::fmt;
use std::sync::Arc;
use tracing::{event, instrument, span, Level};

/// What [`Pipeline::resolve_plan`] predicts for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum PlannedAction {
  /// The step would execute its handlers.
  Run,
  /// The step would be skipped without executing anything.
  Skip(SkipReason),
  /// The step is required but has no handlers: `run` would fail with
  /// [`OrkaError::HandlerMissing`] here.
  FailMissingHandlers,
}

impl fmt::Display for PlannedAction {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      PlannedAction::Run => write!(f, "run"),
      PlannedAction::Skip(reason) => write!(f, "skip ({})", reason),
      PlannedAction::FailMissingHandlers => write!(f, "fail (required step has no handlers)"),
    }
  }
}

/// One entry of the dry-run produced by [`Pipeline::resolve_plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StepPlan {
  pub name: String,
  pub action: PlannedAction,
}

impl fmt::Display for StepPlan {
  /// Renders as `name: action`, so a whole plan prints as a readable preview:
  ///
  /// ```text
  /// prepare: run
  /// drain: skip (drain disabled by config)
  /// install: run
  /// ```
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}: {}", self.name, self.action)
  }
}

fn emit(observer: Option<&Arc<dyn PipelineObserver>>, run_id: u64, kind: TraceEventKind) {
  if let Some(obs) = observer {
    obs.on_event(&TraceEvent { run_id, kind });
  }
}

impl<TData, Err> Pipeline<TData, Err>
where
  TData: 'static + Send + Sync,
  Err: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Executes the pipeline against the given shared context `ctx_data`.
  ///
  /// Returns `Result<PipelineResult, Err>`, where `Err` is the error type
  /// configured for this pipeline's handlers.
  ///
  /// If the pipeline configuration itself leads to an error (e.g., a non-optional
  /// step has no handlers), an `OrkaError` is generated and converted into the
  /// pipeline's `Err` type via its `From<OrkaError>` bound.
  ///
  /// This entry point (and [`run_with_outcome`](Self::run_with_outcome), which it wraps)
  /// runs the [`on_finish`](Self::on_finish) ring: finish handlers are awaited on every
  /// exit (completed, stopped, or errored), after the step loop and before this method
  /// returns. The partial runners ([`run_step`](Self::run_step),
  /// [`run_from`](Self::run_from), [`run_until`](Self::run_until)) never fire them; use
  /// `run()` when you want finish semantics.
  pub async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    self.run_with_outcome(ctx_data).await.0
  }

  /// As [`run`](Self::run), but also returns the [`RunOutcome`], which on failure carries
  /// the name of the step that failed (`Errored { step, message }`). The plain `Err` from
  /// `run()` cannot carry the step without changing the caller's error type; this is the
  /// front door for operator-facing failure reporting ("deploy failed at
  /// 'install-start'") without attaching an observer.
  ///
  /// Identical semantics to `run()` in every other way, including the finish ring: a
  /// finish-handler failure on an otherwise-Ok run yields
  /// `Errored { step: "on_finish", .. }`.
  ///
  /// After the finish handlers, anything stashed in the context's
  /// [`resources`](ContextData::resources) bag is dropped in reverse order.
  #[instrument(
        name = "Pipeline::run",
        skip_all,
        fields(
            pipeline_context_data_type = %std::any::type_name::<TData>(),
            pipeline_error_type = %std::any::type_name::<Err>(),
            num_steps = self.steps.len(),
        ),
    )]
  pub async fn run_with_outcome(&self, ctx_data: ContextData<TData>) -> (Result<PipelineResult, Err>, RunOutcome) {
    self.run_with_observer_inner(ctx_data, None).await
  }

  /// As [`run_with_outcome`](Self::run_with_outcome), with an observer scoped to **this
  /// call** rather than attached to the pipeline.
  ///
  /// [`set_observer`](Self::set_observer) binds an observer to the pipeline, so a
  /// registered pipeline shared by concurrent runs reports all of them into one collector,
  /// and there is no way to learn your own run id up front to filter by: it is allocated
  /// inside the run. This scopes collection to the call you are making.
  ///
  /// A pipeline-attached observer is **not** displaced: both receive every event.
  ///
  /// The scoped observer is inherited by runs started from inside this one, namely fan-out
  /// branches and conditional sub-pipelines, so one collector sees the whole call tree.
  /// That inheritance is deliberately limited to scoped observers; a pipeline-attached one
  /// stays bound to that pipeline's own runs. Note that inheritance gives you *isolation*
  /// (these events are mine) and not *hierarchy*: branch runs carry their own run ids, and
  /// nothing yet records which parent step spawned them.
  pub async fn run_with_observer(
    &self,
    ctx_data: ContextData<TData>,
    observer: Arc<dyn PipelineObserver>,
  ) -> (Result<PipelineResult, Err>, RunOutcome) {
    self.run_with_observer_inner(ctx_data, Some(observer)).await
  }

  async fn run_with_observer_inner(
    &self,
    ctx_data: ContextData<TData>,
    scoped: Option<Arc<dyn PipelineObserver>>,
  ) -> (Result<PipelineResult, Err>, RunOutcome) {
    event!(Level::DEBUG, "Pipeline execution starting.");

    let (run_id, observer, scoped) = self.observe_prologue(scoped);
    emit(observer.as_ref(), run_id, TraceEventKind::RunStarted);

    let result = self
      .run_slice(&self.steps, 0, ctx_data.clone(), run_id, observer.as_ref(), scoped.clone())
      .await;

    let outcome = match &result {
      Ok(PipelineResult::Completed) => RunOutcome::Completed,
      Ok(PipelineResult::Stopped) => RunOutcome::Stopped,
      Err((step, e)) => RunOutcome::Errored {
        step: step.clone(),
        message: e.to_string(),
      },
    };

    // The finish ring: every handler runs, awaited, in registration order, even if one
    // fails. On an Ok run (Completed or Stopped) the first finish-handler error becomes
    // the run's error; on an already-failed run, finish-handler errors are logged and the
    // original error is preserved.
    let mut finish_error: Option<Err> = None;
    for (handler_index, finish_handler) in self.finish_handlers.iter().enumerate() {
      match finish_handler(ctx_data.clone(), outcome.clone()).await {
        Ok(()) => {
          emit(
            observer.as_ref(),
            run_id,
            TraceEventKind::FinalizerFinished {
              handler_index,
              outcome: HandlerOutcome::Continue,
            },
          );
        }
        Err(finish_err) => {
          emit(
            observer.as_ref(),
            run_id,
            TraceEventKind::FinalizerFinished {
              handler_index,
              outcome: HandlerOutcome::Error(finish_err.to_string()),
            },
          );
          if result.is_ok() {
            if finish_error.is_none() {
              finish_error = Some(finish_err);
            } else {
              event!(Level::ERROR, handler_index, error = %finish_err, "Additional finish handler failed; first failure already captured.");
            }
          } else {
            event!(Level::ERROR, handler_index, error = %finish_err, "Finish handler failed on an already-failed run; original error preserved.");
          }
        }
      }
    }

    // Release run-scoped RAII resources, most recently stashed first. This runs after the
    // finish ring on purpose: a finalizer can still copy artifacts out of a temp dir or
    // write a last record under a lock before either is released. It is unconditional,
    // like any Drop, so a failed run releases exactly as a successful one does.
    let released = ctx_data.resources().release_all();
    if released > 0 {
      emit(
        observer.as_ref(),
        run_id,
        TraceEventKind::ResourcesReleased { count: released },
      );
    }

    let final_result: Result<PipelineResult, (String, Err)> = match (result, finish_error) {
      (Ok(r), None) => Ok(r),
      (Ok(_), Some(finish_err)) => Err(("on_finish".to_string(), finish_err)),
      (Err(e), _) => Err(e),
    };

    let final_outcome = match &final_result {
      Ok(PipelineResult::Completed) => RunOutcome::Completed,
      Ok(PipelineResult::Stopped) => RunOutcome::Stopped,
      Err((step, e)) => RunOutcome::Errored {
        step: step.clone(),
        message: e.to_string(),
      },
    };
    emit(
      observer.as_ref(),
      run_id,
      TraceEventKind::RunFinished {
        outcome: final_outcome.clone(),
      },
    );

    match final_result {
      Ok(r) => {
        event!(Level::DEBUG, "Pipeline execution finished.");
        (Ok(r), final_outcome)
      }
      Err((_, e)) => {
        event!(Level::ERROR, error = %e, "Pipeline execution failed.");
        (Err(e), final_outcome)
      }
    }
  }

  /// Executes exactly one step's `before`/`on`/`after` phases against `ctx_data`, with the
  /// step's `skip_if` still respected.
  ///
  /// This is a step-isolation tool for tests: seed a context, run one step, assert the
  /// mutation. It is not a run, so it emits no `RunStarted`/`RunFinished` trace events and
  /// never fires [`on_finish`](Self::on_finish) handlers (which would otherwise consume
  /// exactly the post-step state such a test wants to inspect).
  ///
  /// # Errors
  /// Returns [`OrkaError::StepNotFound`] (converted to `Err`) for an unknown step name.
  pub async fn run_step(&self, step_name: impl AsRef<str>, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    let step_name = step_name.as_ref();
    let idx = self.step_index(step_name)?;
    let (run_id, observer, scoped) = self.observe_prologue(None);
    self
      .run_slice(&self.steps[idx..=idx], idx, ctx_data, run_id, observer.as_ref(), scoped)
      .await
      .map_err(|(_, e)| e)
  }

  /// Executes the pipeline from the named step (inclusive) through the end.
  ///
  /// Like [`run_step`](Self::run_step), this is an inspection tool: no
  /// `RunStarted`/`RunFinished` events, no [`on_finish`](Self::on_finish) handlers.
  ///
  /// # Errors
  /// Returns [`OrkaError::StepNotFound`] (converted to `Err`) for an unknown step name.
  pub async fn run_from(&self, step_name: impl AsRef<str>, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    let step_name = step_name.as_ref();
    let idx = self.step_index(step_name)?;
    let (run_id, observer, scoped) = self.observe_prologue(None);
    self
      .run_slice(&self.steps[idx..], idx, ctx_data, run_id, observer.as_ref(), scoped)
      .await
      .map_err(|(_, e)| e)
  }

  /// Executes the pipeline from the start through the named step (inclusive).
  ///
  /// Like [`run_step`](Self::run_step), this is an inspection tool: no
  /// `RunStarted`/`RunFinished` events, no [`on_finish`](Self::on_finish) handlers.
  ///
  /// # Errors
  /// Returns [`OrkaError::StepNotFound`] (converted to `Err`) for an unknown step name.
  pub async fn run_until(&self, step_name: impl AsRef<str>, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err> {
    let step_name = step_name.as_ref();
    let idx = self.step_index(step_name)?;
    let (run_id, observer, scoped) = self.observe_prologue(None);
    self
      .run_slice(&self.steps[..=idx], 0, ctx_data, run_id, observer.as_ref(), scoped)
      .await
      .map_err(|(_, e)| e)
  }

  /// Dry-runs the pipeline's skip logic against a seeded context, executing nothing.
  ///
  /// Evaluates every step's `skip_if` predicate plus the handler-presence checks and
  /// reports what a `run` with this context would do at each step. No handler executes,
  /// nothing is awaited, no trace events are emitted, and
  /// [`on_finish`](Self::on_finish) handlers do not fire; the context is only passed to
  /// the (pure, synchronous) skip predicates.
  ///
  /// Caveat: all predicates are evaluated against this one static context. A real run's
  /// step-to-step data flow (a step that sets a flag a later predicate reads) is not
  /// simulated, which is exactly why this fits seeded table tests and serves only as a
  /// preview in production.
  pub fn resolve_plan(&self, ctx_data: &ContextData<TData>) -> Vec<StepPlan> {
    self
      .steps
      .iter()
      .map(|step_def| {
        let action = if step_def
          .skip_if
          .as_ref()
          .is_some_and(|cond| cond(ctx_data.clone()))
        {
          PlannedAction::Skip(SkipReason::SkipCondition {
            label: step_def.skip_label.clone(),
          })
        } else if !self.step_has_any_handlers(&step_def.name) {
          if step_def.optional {
            PlannedAction::Skip(SkipReason::OptionalWithoutHandlers)
          } else {
            PlannedAction::FailMissingHandlers
          }
        } else {
          PlannedAction::Run
        };
        StepPlan {
          name: step_def.name.clone(),
          action,
        }
      })
      .collect()
  }


  /// Returns the run id, the observer every event for this run goes to, and the scoped
  /// observer to make ambient for handlers so nested runs inherit it.
  ///
  /// A run started from inside a handler picks up the enclosing call's scoped observer, so
  /// fan-out branches and conditional sub-pipelines report into their parent's collector
  /// without any plumbing at the call site.
  fn observe_prologue(&self, scoped: Option<SharedObserver>) -> RunObservation {
    // One short lock to snapshot the attached observer; never touched again for this run,
    // and no guard survives past this statement.
    let attached = self.observer.lock().clone();
    let scoped = scoped.or_else(current_scoped_observer);
    (next_run_id(), combine_observers(attached, scoped.clone()), scoped)
  }

  fn step_index(&self, step_name: impl AsRef<str>) -> Result<usize, Err> {
    let step_name = step_name.as_ref();
    self
      .steps
      .iter()
      .position(|s| s.name == step_name)
      .ok_or_else(|| {
        Err::from(OrkaError::StepNotFound {
          step_name: step_name.to_string(),
        })
      })
  }

  fn step_has_any_handlers(&self, step_name: impl AsRef<str>) -> bool {
    let step_name = step_name.as_ref();
    [&self.before, &self.on, &self.after]
      .iter()
      .any(|m| m.get(step_name).is_some_and(|v| !v.is_empty()))
  }

  /// The step loop shared by `run` and the partial runners. Errors carry the failing step
  /// name so `run` can build its `RunOutcome` without re-deriving it.
  async fn run_slice(
    &self,
    steps: &[StepDef<TData>],
    index_offset: usize,
    ctx_data: ContextData<TData>,
    run_id: u64,
    observer: Option<&Arc<dyn PipelineObserver>>,
    scoped: Option<Arc<dyn PipelineObserver>>,
  ) -> Result<PipelineResult, (String, Err)> {
    for (slice_idx, step_def) in steps.iter().enumerate() {
      let step_idx = index_offset + slice_idx;
      let step_name_str = step_def.name.as_str();

      let step_span = span!(
        Level::INFO,
        "pipeline_step_execution",
        step_name = step_name_str,
        step_index = step_idx,
        optional = step_def.optional
      );
      let _step_span_guard = step_span.enter();
      event!(Level::DEBUG, "Processing step.");

      if let Some(skip_cond_fn) = &step_def.skip_if
        && skip_cond_fn(ctx_data.clone())
      {
        event!(Level::INFO, "Step skipped due to 'skip_if' condition.");
        emit(
          observer,
          run_id,
          TraceEventKind::StepSkipped {
            step: step_def.name.clone(),
            index: step_idx,
            reason: SkipReason::SkipCondition {
              label: step_def.skip_label.clone(),
            },
          },
        );
        continue;
      }

      if !self.step_has_any_handlers(step_name_str) {
        if step_def.optional {
          event!(Level::DEBUG, "Optional step has no handlers, skipping.");
          emit(
            observer,
            run_id,
            TraceEventKind::StepSkipped {
              step: step_def.name.clone(),
              index: step_idx,
              reason: SkipReason::OptionalWithoutHandlers,
            },
          );
          continue;
        } else {
          event!(Level::ERROR, "Non-optional step has no handlers.");
          let missing = Err::from(OrkaError::HandlerMissing {
            step_name: step_def.name.clone(),
          });
          return Err((step_def.name.clone(), missing));
        }
      }

      emit(
        observer,
        run_id,
        TraceEventKind::StepStarted {
          step: step_def.name.clone(),
          index: step_idx,
        },
      );

      for phase in [StepPhase::Before, StepPhase::On, StepPhase::After] {
        let handlers = match phase {
          StepPhase::Before => self.before.get(step_name_str),
          StepPhase::On => self.on.get(step_name_str),
          StepPhase::After => self.after.get(step_name_str),
        };
        let Some(handlers) = handlers.filter(|v| !v.is_empty()) else {
          continue;
        };

        event!(Level::TRACE, %phase, "Executing handlers.");
        for (handler_idx, handler_fn) in handlers.iter().enumerate() {
          let handler_span = span!(Level::DEBUG, "step_handler", %phase, handler_index = handler_idx);
          let _handler_span_guard = handler_span.enter();

          // HandlerScope makes the run id and the call-scoped observer ambient for the
          // duration of each poll, so code executing inside the handler can tag its events
          // with this run (the conditional master handler) and any run it starts inherits
          // the collector (fan-out branches).
          let handler_result = HandlerScope {
            run_id,
            scoped_observer: scoped.clone(),
            fut: handler_fn(ctx_data.clone()),
          }
          .await;

          match handler_result {
            Ok(PipelineControl::Continue) => {
              emit(
                observer,
                run_id,
                TraceEventKind::HandlerFinished {
                  step: step_def.name.clone(),
                  phase,
                  handler_index: handler_idx,
                  outcome: HandlerOutcome::Continue,
                },
              );
            }
            Ok(PipelineControl::Stop) => {
              event!(Level::INFO, %phase, "Pipeline stopped by a handler.");
              emit(
                observer,
                run_id,
                TraceEventKind::HandlerFinished {
                  step: step_def.name.clone(),
                  phase,
                  handler_index: handler_idx,
                  outcome: HandlerOutcome::Stop,
                },
              );
              return Ok(PipelineResult::Stopped);
            }
            Err(e) => {
              event!(Level::ERROR, %phase, error = %e, "Handler failed.");
              if let Some(obs) = observer {
                obs.on_handler_error(run_id, step_name_str, phase, &e);
              }
              emit(
                observer,
                run_id,
                TraceEventKind::HandlerFinished {
                  step: step_def.name.clone(),
                  phase,
                  handler_index: handler_idx,
                  outcome: HandlerOutcome::Error(e.to_string()),
                },
              );
              return Err((step_def.name.clone(), e));
            }
          }
        }
      }

      emit(
        observer,
        run_id,
        TraceEventKind::StepCompleted {
          step: step_def.name.clone(),
          index: step_idx,
        },
      );
      event!(Level::DEBUG, "Step processing finished successfully.");
    }

    Ok(PipelineResult::Completed)
  }
}
