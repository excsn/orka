# Orka Workflow Engine - API Reference

## 1. Introduction / Core Concepts

Orka is an asynchronous, pluggable, and type-safe workflow engine for Rust. It allows developers to define complex, multi-step processes (pipelines) with fine-grained control over execution flow, error handling, and context management.

**Core Concepts & Primary Structs:**

*   **`Pipeline<TData, Err>`:** The central construct representing a workflow. It's generic over:
    *   `TData`: The primary data type for the pipeline's shared context. Must be `'static + Send + Sync`.
    *   `Err`: The error type returned by this pipeline's handlers. Must be `std::error::Error + From<OrkaError> + Send + Sync + 'static`.
    It manages a sequence of named steps, and handlers can be registered for `before`, `on`, and `after` phases of each step.

*   **`ContextData<T>`:** A smart-pointer wrapper (`Arc<RwLock<T>>`) providing shared, mutable access to context data within and across pipeline handlers. Clones of `ContextData<T>` share the same underlying data. **Lock guards obtained from `ContextData` must be dropped before any `.await` suspension point.**

*   **`Handler<TData, Err>` (Type Alias):** Represents an asynchronous function executed as part of a pipeline step. It takes `ContextData<TData>` and returns a `Future` resolving to `Result<PipelineControl, Err>`.

*   **`ConditionalScopeBuilder<'pipeline, TData, Err>`:** A fluent builder API used to define conditional execution of "scoped" sub-pipelines within a step of a main pipeline. This allows for dynamic branching of workflows.

*   **Scoped Pipelines:** These are independent `Pipeline<SData, Err>` instances (where `Err` is the main pipeline's error type) that can be executed conditionally. They operate on an extracted sub-context `SData`.

*   **`PipelineProvider<TData, SData, MainErr>` (Trait):** Defines how scoped pipelines are sourced, either statically or dynamically via a factory.

*   **`Orka<ApplicationError>`:** A type-keyed registry for managing and running different `Pipeline` instances. It's generic over `ApplicationError`, which is the top-level error type returned by `Orka::run`. `ApplicationError` must be `From<OrkaError>`.

*   **`OrkaError` (Enum):** The primary error type for the Orka framework itself, covering issues like configuration errors, missing handlers, or internal problems. Application-level error types should be `From<OrkaError>`.

**Main Entry Points:**

*   Creating a `Pipeline::new(...)`.
*   Registering handlers using methods like `pipeline.on_root(...)`, `pipeline.before_root(...)`, `pipeline.after_root(...)`.
*   Defining conditional logic using `pipeline.conditional_scopes_for_step(...)` to get a `ConditionalScopeBuilder`.
*   Creating an `Orka::new()` registry.
*   Registering pipelines with the registry: `orka_instance.register_pipeline(pipeline)`.
*   Executing a registered pipeline: `orka_instance.run(context_data).await`.

**Pervasive Types/Patterns:**

*   **`OrkaResult<T, E = OrkaError>` (Type Alias):** The standard `Result` type used for Orka's internal operations, defaulting to `OrkaError`.
*   **`ContextData<T>`:** Used ubiquitously for passing shared state to handlers.
*   **`PipelineControl` (Enum):** Returned by handlers to signal whether the pipeline should continue or stop.
*   **`PipelineResult` (Enum):** The outcome of a full pipeline execution (Completed or Stopped).
*   **`From<OrkaError>` Trait Bound:** Frequently required for application-specific error types used with `Pipeline` or `Orka` to allow them to absorb framework-level errors.

## 2. Main Types and Their Public Methods

### Struct `orka::pipeline::definition::Pipeline<TData, Err>`

The core workflow definition.

**Generic Parameters:**

*   `TData: 'static + Send + Sync`
*   `Err: std::error::Error + From<crate::error::OrkaError> + Send + Sync + 'static`

**Public Methods:**

*   **`pub fn new(step_defs: &[(&str, bool, Option<SkipCondition<TData>>)]) -> Self`**
    *   Creates a new pipeline with an initial set of step definitions.
    *   `SkipCondition<TData>` is `std::sync::Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>`.

*   **`pub fn insert_before_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S, optional: bool, skip_if: Option<SkipCondition<TData>>)`**
    *   Inserts a new step definition before an existing step. Panics if `existing_step_name` is not found or if `new_step_name` already exists.

*   **`pub fn insert_after_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S, optional: bool, skip_if: Option<SkipCondition<TData>>)`**
    *   Inserts a new step definition after an existing step. Panics if `existing_step_name` is not found or if `new_step_name` already exists.

*   **`pub fn remove_step(&mut self, step_name: &str)`**
    *   Removes a step definition and its associated handlers/configurations. No-op if the step is not found.

*   **`pub fn set_optional(&mut self, step_name: &str, optional: bool)`**
    *   Sets the optional flag for a given step. Panics if `step_name` is not found.

*   **`pub fn set_skip_condition(&mut self, step_name: &str, skip_if: Option<SkipCondition<TData>>)`**
    *   Sets or clears the skip condition for a given step. Panics if `step_name` is not found.

*   **`pub fn before_root<F, UserProvidedErr>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static)`**
    *   Where:
        *   `F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static`
        *   `UserProvidedErr: Into<Err> + Send + Sync + 'static`
    *   Registers a handler to be executed *before* the main `on` handlers for the specified step.

*   **`pub fn on_root<F, UserProvidedErr>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static)`**
    *   Where:
        *   `F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static`
        *   `UserProvidedErr: Into<Err> + Send + Sync + 'static`
    *   Registers a main handler for the specified step.

*   **`pub fn after_root<F, UserProvidedErr>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static)`**
    *   Where:
        *   `F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static`
        *   `UserProvidedErr: Into<Err> + Send + Sync + 'static`
    *   Registers a handler to be executed *after* the main `on` handlers for the specified step.

*   **`pub fn set_extractor<SData>(&mut self, step_name: &str, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static)`**
    *   Where:
        *   `SData: 'static + Send + Sync`
    *   Registers an extractor function for a step, allowing subsequent handlers (registered with `on<SData>`) to operate on a sub-context `ContextData<SData>`. The extractor itself returns `Result<_, OrkaError>` as extraction failure is a framework concern.

*   **`pub fn on<SData, F, SubHandlerErr>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static)`**
    *   Where:
        *   `SData: 'static + Send + Sync`
        *   `F: Future<Output = Result<PipelineControl, SubHandlerErr>> + Send + 'static`
        *   `SubHandlerErr: Into<Err> + Send + Sync + 'static + std::fmt::Debug`
        *   *(Implicitly `Err: From<OrkaError>` from the `impl` block)*
    *   Registers a handler that operates on an extracted sub-context `ContextData<SData>`. An extractor must be set first using `set_extractor`. Errors from the extractor (`OrkaError`) are converted to `Err`.

*   **`pub fn conditional_scopes_for_step(&mut self, step_name: &str) -> ConditionalScopeBuilder<TData, Err>`**
    *   Returns a builder to define conditional execution of scoped sub-pipelines for the specified step. The step will be created if it doesn't exist.

*   **`pub async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>`**
    *   Executes the entire pipeline sequentially.
    *   Internal Orka framework errors during execution (e.g., missing handler for a non-optional step) are converted to `Err`.

### Struct `orka::core::context_data::ContextData<T>`

A wrapper for shared context data using `Arc<RwLock<T>>`.

**Generic Parameters:**

*   `T: 'static + Send + Sync`

**Public Methods:**

*   **`pub fn new(data: T) -> Self`**
    *   Creates a new `ContextData` instance wrapping the given data.

*   **`pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, T>`**
    *   Acquires a read lock. Panics if poisoned. Guard must be dropped before `.await`.

*   **`pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, T>`**
    *   Acquires a write lock. Panics if poisoned. Guard must be dropped before `.await`.

*   **`pub fn try_read(&self) -> Option<parking_lot::RwLockReadGuard<'_, T>>`**
    *   Attempts to acquire a read lock without blocking.

*   **`pub fn try_write(&self) -> Option<parking_lot::RwLockWriteGuard<'_, T>>`**
    *   Attempts to acquire a write lock without blocking.

*   **`pub fn map_read<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockReadGuard<'_, U>`**
    *   Where `F: FnOnce(&T) -> &U`
    *   Acquires a read lock and maps it to a part of the data.

*   **`pub fn map_write<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockWriteGuard<'_, U>`**
    *   Where `F: FnOnce(&mut T) -> &mut U`
    *   Acquires a write lock and maps it to a part of the data.

**Implemented Traits:** `Clone`, `Debug`, `Default` (if `T: Default`).

### Struct `orka::conditional::builder::ConditionalScopeBuilder<'pipeline, TData, Err>`

Builder for defining conditional scopes within a pipeline step.

**Generic Parameters:**

*   `'pipeline` (lifetime)
*   `TData: 'static + Send + Sync`
*   `Err: std::error::Error + From<OrkaError> + Send + Sync + 'static`

**Public Methods:**

*   **`pub fn add_static_scope<SData>(self, static_pipeline: Arc<Pipeline<SData, Err>>, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, StaticPipelineProvider<SData, Err>>`**
    *   Where:
        *   `SData: 'static + Send + Sync`
    *   Adds a scope that uses a statically provided `Pipeline<SData, Err>`. The `extractor_fn` produces the sub-context, and its potential failure is an `OrkaError`.

*   **`pub fn add_dynamic_scope<SData, F, Fut>(self, pipeline_factory: F, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, FunctionalPipelineProvider<TData, SData, Err, F, Fut>>`**
    *   Where:
        *   `SData: 'static + Send + Sync`
        *   `F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static`
        *   `Fut: Future<Output = Result<Arc<Pipeline<SData, Err>>, OrkaError>> + Send + 'static`
    *   Adds a scope that uses an asynchronous factory function to obtain a `Pipeline<SData, Err>`. The factory itself can fail with an `OrkaError`. The `extractor_fn` can also fail with an `OrkaError`.

*   **`pub fn if_no_scope_matches(mut self, behavior: PipelineControl) -> Self`**
    *   Specifies the `PipelineControl` behavior if no conditional scopes match during execution. Defaults to `PipelineControl::Continue`.

*   **`pub fn finalize_conditional_step(self, optional_for_main_step: bool)`**
    *   Finalizes the conditional scopes for the current step, registers a master handler in the main pipeline, and sets the optionality of this conditional step within the main pipeline.

### Struct `orka::conditional::builder::ConditionalScopeConfigurator<'pipeline, TData, SData, Err, P>`

Intermediate builder for configuring a single conditional scope (setting its condition).

**Generic Parameters:**

*   `'pipeline` (lifetime)
*   `TData: 'static + Send + Sync`
*   `SData: 'static + Send + Sync`
*   `Err: std::error::Error + From<OrkaError> + Send + Sync + 'static`
*   `P: PipelineProvider<TData, SData, Err> + 'static`

**Public Methods:**

*   **`pub fn on_condition(mut self, condition_fn: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> ConditionalScopeBuilder<'pipeline, TData, Err>`**
    *   Sets the boolean condition for the current scope to be executed. Returns the main `ConditionalScopeBuilder` to allow chaining or adding more scopes.

### Struct `orka::registry::Orka<ApplicationError = OrkaError>`

A type-keyed registry for managing and executing different `Pipeline` instances.

**Generic Parameters:**

*   `ApplicationError: std::error::Error + From<OrkaError> + Send + Sync + 'static` (defaults to `OrkaError`)

**Public Methods:**

*   **`pub fn new() -> Self`**
    *   Creates a new, empty Orka registry.

*   **`pub fn new_default() -> Orka<OrkaError>`**
    *   Convenience method to create `Orka` with `OrkaError` as its application error type. *(Only available if `ApplicationError` is `OrkaError`)*.

*   **`pub fn register_pipeline<TData, PipelineHandlerError>(&self, pipeline: Pipeline<TData, PipelineHandlerError>)`**
    *   Where:
        *   `TData: 'static + Send + Sync`
        *   `PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static`
        *   `ApplicationError: From<PipelineHandlerError>`
        *   `Pipeline<TData, PipelineHandlerError>: Send + Sync`
    *   Registers a pipeline with the Orka instance. It will be keyed by the `TypeId` of `TData`.

*   **`pub async fn run<TData>(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, ApplicationError>`**
    *   Where:
        *   `TData: 'static + Send + Sync`
    *   Executes the pipeline registered for the type `TData` using the provided `ctx_data`. Returns the outcome of the pipeline or an `ApplicationError`.

## 3. Public Traits and Their Methods

### Trait `orka::conditional::provider::PipelineProvider<TData, SData, MainErr>`

Defines a contract for objects that can provide instances of scoped pipelines.

**Generic Parameters:**

*   `TData: 'static + Send + Sync`
*   `SData: 'static + Send + Sync`
*   `MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static`

**Methods:**

*   **`async fn get_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<Arc<Pipeline<SData, MainErr>>, OrkaError>`**
    *   Asynchronously gets or creates an `Arc<Pipeline<SData, MainErr>>`. The provider's own operation can fail with an `OrkaError`.

## 4. Public Enums (Non-Config)

### Enum `orka::core::control::PipelineControl`

Signal from a handler indicating whether the pipeline should continue or stop.

**Variants:**

*   **`Continue`**
*   **`Stop`**

### Enum `orka::core::control::PipelineResult`

Outcome of a full pipeline execution.

**Variants:**

*   **`Completed`**: The pipeline executed all non-skipped, non-optional steps.
*   **`Stopped`**: The pipeline was explicitly stopped by a handler.

## 5. Public Type Aliases

### `pub type Handler<TData, Err> = Box<dyn Fn(ContextData<TData>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PipelineControl, Err>> + Send>> + Send + Sync>`
*   (Located in `orka::core::context`)
*   The type for a pipeline step handler function.

### `pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>`
*   (Located in `orka::error`)
*   A standard result type defaulting to `OrkaError` for the error variant.

### `pub type SkipCondition<TData> = std::sync::Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>`
*   (Located in `orka::core::step`)
*   Type for a closure used to determine if a pipeline step should be skipped.

## 6. Error Handling

### Enum `orka::error::OrkaError`

The primary error type for the Orka framework.

**Variants:**

*   **`StepNotFound { step_name: String }`**
*   **`HandlerMissing { step_name: String }`**
*   **`ExtractorFailure { step_name: String, source: anyhow::Error }`**
*   **`PipelineProviderFailure { step_name: String, source: anyhow::Error }`**
*   **`TypeMismatch { step_name: String, expected_type: String }`**
*   **`HandlerError { source: anyhow::Error }`**: Wraps an error originating from user-provided handler logic or external operations, typically when an external error is converted to `OrkaError`.
*   **`ConfigurationError { step_name: String, message: String }`**
*   **`Internal(String)`**: For miscellaneous internal Orka errors.
*   **`NoConditionalScopeMatched { step_name: String }`**: Used if conditional execution completes without any scope's condition being met and no default behavior is specified to handle this scenario (though currently, `ConditionalScopeBuilder` defaults to `Continue` or user-specified control).

**Standard Result Type:**

*   **`pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;`**
    *   Applications using Orka will typically define their own error enum (e.g., `AppError`) and implement `From<OrkaError>` for it, allowing seamless conversion of Orka framework errors into the application's error domain.

## 7. Modules

The public API is primarily exposed through re-exports in `orka::lib.rs`. Key modules include:

*   **`orka::pipeline`**: Contains `Pipeline` definition.
*   **`orka::core`**: Contains core types like `ContextData`, `Handler`, `PipelineControl`, `PipelineResult`, `StepDef`.
*   **`orka::conditional`**: Contains `ConditionalScopeBuilder`, `ConditionalScopeConfigurator`, and `PipelineProvider` trait.
*   **`orka::registry`**: Contains the `Orka` registry.
*   **`orka::error`**: Contains `OrkaError` and `OrkaResult`.