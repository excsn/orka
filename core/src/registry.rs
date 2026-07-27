//! Defines the `Orka<E>` struct, a type-keyed registry for managing and executing pipelines.
//! Pipelines are `crate::pipeline::definition::Pipeline<TData, PipelineHandlerError>`.
//! The registry returns results with an application-level error type `E`.

use crate::core::control::PipelineResult;
use crate::core::context_data::ContextData;
use crate::core::trace::RunOutcome;
use crate::error::OrkaError;
use crate::pipeline::definition::Pipeline as CorePipeline;

use crate::error::OrkaResult;
use crate::pipeline::runner::PipelineRunner;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::{event, instrument, Level};

/// Type-erased trait for pipeline execution by the registry.
/// `ApplicationError` is the error type returned by `Orka::run`.
#[async_trait]
trait AnyPipelineRunner<ApplicationError>: Send + Sync
where
  ApplicationError: std::error::Error + Send + Sync + 'static,
{
  /// Executes the pipeline with a type-erased, owned context.
  /// `ctx_obj` is expected to be a `Box<dyn Any + Send>` containing `ContextData<TData>`.
  async fn run_any_erased_with_owned_ctx(&self, ctx_obj: Box<dyn Any + Send>) -> Result<PipelineResult, ApplicationError>;

  /// As `run_any_erased_with_owned_ctx`, additionally returning the run's outcome for
  /// [`Orka::run_with_outcome`].
  async fn run_any_erased_detailed(
    &self,
    ctx_obj: Box<dyn Any + Send>,
  ) -> (Result<PipelineResult, ApplicationError>, RunOutcome);

  /// Downcast support for [`Orka::pipeline`], which recovers the concrete wrapper.
  fn as_any(&self) -> &dyn Any;
}

/// Wrapper making a `PipelineRunner<TData, PipelineHandlerError>` runnable by
/// `Orka<ApplicationError>`. When the registration came in as a concrete pipeline
/// (`register_pipeline`), the wrapper also keeps the concrete `Arc<Pipeline>` so
/// [`Orka::pipeline`] can hand it back; runner-only registrations (`register_runner`)
/// leave it `None`.
struct PipelineWrapper<TData, PipelineHandlerError, ApplicationError>
where
  TData: 'static + Send + Sync,
  PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static, // Must be From<OrkaError> for pipeline.run
  ApplicationError: std::error::Error + From<PipelineHandlerError> + From<OrkaError> + Send + Sync + 'static,
  CorePipeline<TData, PipelineHandlerError>: Send + Sync,
{
  runner: Arc<dyn PipelineRunner<TData, PipelineHandlerError>>,
  pipeline: Option<Arc<CorePipeline<TData, PipelineHandlerError>>>,
  _phantom_app_err: PhantomData<ApplicationError>,
}

#[async_trait]
impl<TData, PipelineHandlerError, ApplicationError> AnyPipelineRunner<ApplicationError>
  for PipelineWrapper<TData, PipelineHandlerError, ApplicationError>
where
  TData: 'static + Send + Sync,
  PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
  ApplicationError: std::error::Error + From<PipelineHandlerError> + From<OrkaError> + Send + Sync + 'static,
  CorePipeline<TData, PipelineHandlerError>: Send + Sync,
{
  #[instrument(
        name = "PipelineWrapper::run_any_erased_with_owned_ctx",
        skip_all,
        fields(
            target_tdata_type = %std::any::type_name::<TData>(),
            pipeline_handler_error_type = %std::any::type_name::<PipelineHandlerError>(),
            application_error_type = %std::any::type_name::<ApplicationError>(),
        ),
        err(Display)
    )]
  async fn run_any_erased_with_owned_ctx(&self, ctx_obj: Box<dyn Any + Send>) -> Result<PipelineResult, ApplicationError> {
    event!(Level::TRACE, "Attempting to downcast owned context object.");

    let typed_ctx_data = match ctx_obj.downcast::<ContextData<TData>>() {
      Ok(boxed_ctx_data) => *boxed_ctx_data,
      Err(_) => {
        let expected_type_name = std::any::type_name::<ContextData<TData>>();
        event!(Level::ERROR, "Context object type mismatch. Expected {}.", expected_type_name);
        let orka_type_mismatch = OrkaError::TypeMismatch {
            step_name: "registry_dispatch".to_string(),
            expected_type: expected_type_name.to_string(),
        };
        return Err(ApplicationError::from(orka_type_mismatch));
      }
    };

    event!(Level::DEBUG, "Context object downcast successful. Executing wrapped pipeline.");
    self.runner.run(typed_ctx_data).await.map_err(ApplicationError::from)
  }

  async fn run_any_erased_detailed(
    &self,
    ctx_obj: Box<dyn Any + Send>,
  ) -> (Result<PipelineResult, ApplicationError>, RunOutcome) {
    let typed_ctx_data = match ctx_obj.downcast::<ContextData<TData>>() {
      Ok(boxed_ctx_data) => *boxed_ctx_data,
      Err(_) => {
        let orka_type_mismatch = OrkaError::TypeMismatch {
          step_name: "registry_dispatch".to_string(),
          expected_type: std::any::type_name::<ContextData<TData>>().to_string(),
        };
        let outcome = RunOutcome::Errored {
          step: "registry_dispatch".to_string(),
          message: orka_type_mismatch.to_string(),
        };
        return (Err(ApplicationError::from(orka_type_mismatch)), outcome);
      }
    };

    let (result, outcome) = self.runner.run_with_outcome(typed_ctx_data).await;
    (result.map_err(ApplicationError::from), outcome)
  }

  fn as_any(&self) -> &dyn Any {
    self
  }
}

/// The Orka registry.
/// `ApplicationError` is the error type that `Orka::run` will return.
/// This error type must be constructible from `OrkaError` to handle internal
/// framework errors (e.g., pipeline not found, type mismatches).
pub struct Orka<ApplicationError = OrkaError>
where
  ApplicationError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  registry: Mutex<HashMap<TypeId, Arc<dyn AnyPipelineRunner<ApplicationError>>>>,
  _phantom_app_err: PhantomData<ApplicationError>,
}

impl<ApplicationError> Orka<ApplicationError>
where
  ApplicationError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  /// Creates a new, empty Orka registry.
  pub fn new() -> Self {
    Self {
      registry: Mutex::new(HashMap::new()),
      _phantom_app_err: PhantomData,
    }
  }

  /// Registers a `CorePipeline<TData, PipelineHandlerError>` with the Orka registry.
  ///
  /// - `PipelineHandlerError` (used by the pipeline's handlers) must be `From<OrkaError>`.
  /// - `ApplicationError` (this registry's error type) must be `From<PipelineHandlerError>`.
  ///
  /// The pipeline is [`validate`](CorePipeline::validate)d first, so setup mistakes surface
  /// here rather than on the first run.
  ///
  /// Pipelines are keyed by `TData`, so registering a second pipeline for the same context
  /// type replaces the first.
  ///
  /// # Errors
  /// Returns [`OrkaError::ConfigurationError`] if the pipeline fails validation.
  pub fn register_pipeline<TData, PipelineHandlerError>(
    &self,
    pipeline: CorePipeline<TData, PipelineHandlerError>,
  ) -> OrkaResult<()>
  where
    TData: 'static + Send + Sync,
    PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static, // Bound for Pipeline::run
    ApplicationError: From<PipelineHandlerError>, // Bound for PipelineWrapper
    CorePipeline<TData, PipelineHandlerError>: Send + Sync,
  {
    event!(Level::DEBUG, tdata_type = %std::any::type_name::<TData>(), pipeline_handler_error = %std::any::type_name::<PipelineHandlerError>(), "Registering pipeline.");

    pipeline.validate()?;

    // One allocation: the same Arc is stored twice, once as the runner (upcast) and once
    // concretely so `Orka::pipeline` can return it.
    let pipeline = Arc::new(pipeline);
    let wrapper = PipelineWrapper::<TData, PipelineHandlerError, ApplicationError> {
      runner: pipeline.clone(),
      pipeline: Some(pipeline),
      _phantom_app_err: PhantomData,
    };
    self
      .registry
      .lock()
      .insert(TypeId::of::<TData>(), Arc::new(wrapper));

    Ok(())
  }

  /// Registers any [`PipelineRunner`] (a `test_util::MockPipeline`, a middleware wrapper
  /// around a real pipeline) under `TData`.
  ///
  /// No validation runs; the runner is trusted as-is. Replaces any previous registration
  /// for the same `TData`, exactly like [`register_pipeline`](Self::register_pipeline);
  /// registering over an existing entry is the intended way to swap in an instrumented or
  /// mock variant. [`pipeline`](Self::pipeline) returns `None` for registrations made this
  /// way, since there is no concrete `Pipeline` to hand back.
  pub fn register_runner<TData, PipelineHandlerError>(
    &self,
    runner: Arc<dyn PipelineRunner<TData, PipelineHandlerError>>,
  ) where
    TData: 'static + Send + Sync,
    PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
    ApplicationError: From<PipelineHandlerError>,
    CorePipeline<TData, PipelineHandlerError>: Send + Sync,
  {
    event!(Level::DEBUG, tdata_type = %std::any::type_name::<TData>(), pipeline_handler_error = %std::any::type_name::<PipelineHandlerError>(), "Registering runner.");

    let wrapper = PipelineWrapper::<TData, PipelineHandlerError, ApplicationError> {
      runner,
      pipeline: None,
      _phantom_app_err: PhantomData,
    };
    self
      .registry
      .lock()
      .insert(TypeId::of::<TData>(), Arc::new(wrapper));
  }

  /// The registered pipeline for `TData`, if it was registered as a concrete pipeline via
  /// [`register_pipeline`](Self::register_pipeline).
  /// [`register_runner`](Self::register_runner)-only registrations (mocks, middleware)
  /// honestly return `None`, as does a `PipelineHandlerError` type parameter that does not
  /// match the registration.
  ///
  /// This makes the registry itself the test scope: everything that matters on a
  /// registered pipeline is reachable through `&self`, so a test (or a production
  /// diagnostic) can attach a tracer, dry-run `resolve_plan`, or run a single step
  /// against the actually-registered pipeline with no parallel construction:
  ///
  /// ```ignore
  /// let orka = build_registry()?;                 // the same wiring production uses
  /// let p = orka.pipeline::<MyCtx, MyErr>().unwrap();
  /// p.set_tracer(trace.clone());
  /// let plan = p.resolve_plan(&seeded_ctx);
  /// ```
  ///
  /// The `&mut self` override methods (`stub_step`, `replace_on_root`, ...) are
  /// deliberately not reachable through this accessor: a live registered pipeline should
  /// not be mutated under concurrent runs. For those, build the pipeline via your
  /// production build function, mutate while you still hold it `&mut`, and register it
  /// into a fresh `Orka`.
  pub fn pipeline<TData, PipelineHandlerError>(&self) -> Option<Arc<CorePipeline<TData, PipelineHandlerError>>>
  where
    TData: 'static + Send + Sync,
    PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
    ApplicationError: From<PipelineHandlerError>,
    CorePipeline<TData, PipelineHandlerError>: Send + Sync,
  {
    let registry = self.registry.lock();
    let runner = registry.get(&TypeId::of::<TData>())?;
    let wrapper = runner
      .as_any()
      .downcast_ref::<PipelineWrapper<TData, PipelineHandlerError, ApplicationError>>()?;
    wrapper.pipeline.clone()
  }

  /// Runs the pipeline registered for the underlying data type `TData`.
  pub async fn run<TData>(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, ApplicationError>
  where
    TData: 'static + Send + Sync,
  {
    event!(Level::DEBUG, tdata_type = %std::any::type_name::<TData>(), "Attempting to run pipeline.");
    let type_id = TypeId::of::<TData>();

    let runner_arc: Arc<dyn AnyPipelineRunner<ApplicationError>>;
    {
      let reg_lock = self.registry.lock();
      runner_arc = reg_lock
        .get(&type_id)
        .cloned()
        .ok_or_else(|| {
          let type_name = std::any::type_name::<TData>();
          event!(Level::ERROR, "No pipeline registered for TData type {}.", type_name);
          let orka_config_err = OrkaError::ConfigurationError {
            step_name: "Orka::run".to_string(),
            message: format!("No pipeline registered for TData type {}", type_name),
          };
          ApplicationError::from(orka_config_err)
        })?;
    }

    let owned_ctx_obj: Box<dyn Any + Send> = Box::new(ctx_data.clone());
    runner_arc.run_any_erased_with_owned_ctx(owned_ctx_obj).await
  }

  /// As [`run`](Self::run), but also returns the [`RunOutcome`], which on failure carries
  /// the failing step's name. This is what a job shell wants for operator-facing failure
  /// reporting ("deploy failed at 'install-start'") when the application error type
  /// cannot carry the step itself.
  ///
  /// For registrations made via [`register_runner`](Self::register_runner) whose runner
  /// does not override `PipelineRunner::run_with_outcome`, the outcome's `step` is empty
  /// on failure (the runner cannot attribute it). A missing registration yields
  /// `Errored { step: "Orka::run", .. }`.
  pub async fn run_with_outcome<TData>(
    &self,
    ctx_data: ContextData<TData>,
  ) -> (Result<PipelineResult, ApplicationError>, RunOutcome)
  where
    TData: 'static + Send + Sync,
  {
    let type_id = TypeId::of::<TData>();
    let runner_arc = { self.registry.lock().get(&type_id).cloned() };

    let Some(runner_arc) = runner_arc else {
      let type_name = std::any::type_name::<TData>();
      let orka_config_err = OrkaError::ConfigurationError {
        step_name: "Orka::run".to_string(),
        message: format!("No pipeline registered for TData type {}", type_name),
      };
      let outcome = RunOutcome::Errored {
        step: "Orka::run".to_string(),
        message: orka_config_err.to_string(),
      };
      return (Err(ApplicationError::from(orka_config_err)), outcome);
    };

    let owned_ctx_obj: Box<dyn Any + Send> = Box::new(ctx_data.clone());
    runner_arc.run_any_erased_detailed(owned_ctx_obj).await
  }
}

impl<ApplicationError> Default for Orka<ApplicationError>
where
  ApplicationError: std::error::Error + From<OrkaError> + Send + Sync + 'static,
{
  fn default() -> Self {
    Self::new()
  }
}

impl Orka<OrkaError> {
  /// Convenience constructor for the common case where `OrkaError` is also the application
  /// error type.
  pub fn new_default() -> Self {
    Orka::<OrkaError>::new()
  }
}