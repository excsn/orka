// orka/src/registry.rs

//! Defines the `Orka<E>` struct, a type-keyed registry for managing and executing pipelines.
//! Pipelines are `crate::pipeline::definition::Pipeline<TData, PipelineHandlerError>`.
//! The registry returns results with an application-level error type `E`.

use crate::core::control::PipelineResult;
use crate::core::context_data::ContextData;
use crate::error::OrkaError; // Orka's own error type
// Explicitly import the main Pipeline definition
use crate::pipeline::definition::Pipeline as CorePipeline;

use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
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
}

/// Wrapper for `CorePipeline<TData, PipelineHandlerError>` to make it runnable by `Orka<ApplicationError>`.
struct PipelineWrapper<TData, PipelineHandlerError, ApplicationError>
where
  TData: 'static + Send + Sync,
  PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static, // Must be From<OrkaError> for pipeline.run
  // ApplicationError must be From PipelineHandlerError AND From OrkaError (for registry-level errors)
  ApplicationError: std::error::Error + From<PipelineHandlerError> + From<OrkaError> + Send + Sync + 'static,
  CorePipeline<TData, PipelineHandlerError>: Send + Sync,
{
  pipeline: Arc<CorePipeline<TData, PipelineHandlerError>>,
  _phantom_tdata: PhantomData<TData>,
  _phantom_handler_err: PhantomData<PipelineHandlerError>,
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
      Ok(boxed_ctx_data) => *boxed_ctx_data, // Unbox to get ContextData<TData>
      Err(_) => { // downcast failed
        let expected_type_name = std::any::type_name::<ContextData<TData>>();
        event!(Level::ERROR, "Context object type mismatch. Expected {}.", expected_type_name);
        // Create an OrkaError for the type mismatch.
        let orka_type_mismatch = OrkaError::TypeMismatch {
            step_name: "registry_dispatch".to_string(),
            expected_type: expected_type_name.to_string(),
        };
        // Convert this OrkaError to the ApplicationError using the existing trait bound.
        return Err(ApplicationError::from(orka_type_mismatch));
      }
    };

    event!(Level::DEBUG, "Context object downcast successful. Executing wrapped pipeline.");
    // self.pipeline.run returns Result<PipelineResult, PipelineHandlerError>.
    // The PipelineHandlerError already satisfies From<OrkaError> due to Pipeline::run's bound.
    // We then map PipelineHandlerError to ApplicationError using From.
    self.pipeline.run(typed_ctx_data).await.map_err(ApplicationError::from)
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
  pub fn register_pipeline<TData, PipelineHandlerError>(
    &self,
    pipeline: CorePipeline<TData, PipelineHandlerError>,
  )
  where
    TData: 'static + Send + Sync,
    PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static, // Bound for Pipeline::run
    ApplicationError: From<PipelineHandlerError>, // Bound for PipelineWrapper
    CorePipeline<TData, PipelineHandlerError>: Send + Sync,
  {
    event!(Level::DEBUG, tdata_type = %std::any::type_name::<TData>(), pipeline_handler_error = %std::any::type_name::<PipelineHandlerError>(), "Registering pipeline.");
    let wrapper = PipelineWrapper::<TData, PipelineHandlerError, ApplicationError> {
      pipeline: Arc::new(pipeline),
      _phantom_tdata: PhantomData,
      _phantom_handler_err: PhantomData,
      _phantom_app_err: PhantomData,
    };
    self
      .registry
      .lock()
      .unwrap()
      .insert(TypeId::of::<TData>(), Arc::new(wrapper)); // Keyed by TData
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
      let reg_lock = self.registry.lock().unwrap();
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
          // Convert OrkaError to ApplicationError using the bound on Orka<ApplicationError>
          ApplicationError::from(orka_config_err)
        })?;
    }

    let owned_ctx_obj: Box<dyn Any + Send> = Box::new(ctx_data.clone());
    runner_arc.run_any_erased_with_owned_ctx(owned_ctx_obj).await
  }
}

impl Orka<OrkaError> {
  pub fn new_default() -> Self {
    Orka::<OrkaError>::new()
  }
}