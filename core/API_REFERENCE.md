# Orka Workflow Engine - API Reference

Covers orka 0.2.

## 1. Introduction / Core Concepts

Orka is an asynchronous, pluggable, and type-safe workflow engine for Rust. It allows developers to define complex, multi-step processes (pipelines) with fine-grained control over execution flow, error handling, and context management.

**Core Concepts & Primary Structs:**

*   **`Pipeline<TData, Err>`:** The central construct representing a workflow. Generic over:
    *   `TData`: The data type for the pipeline's shared context. Must be `'static + Send + Sync`.
    *   `Err`: The error type returned by this pipeline's handlers. Must be `std::error::Error + From<OrkaError> + Send + Sync + 'static`.

    It manages a sequence of named steps; handlers can be registered for the `before`, `on`, and `after` phases of each step.

*   **`ContextData<T>`:** A wrapper (`Arc<RwLock<T>>`) providing shared, mutable access to context data. Clones share the same underlying data. **Lock guards must be dropped before any `.await` suspension point.**

*   **`Handler<TData, Err>` (Type Alias):** An asynchronous function executed as part of a step. Takes `ContextData<TData>` and returns a future resolving to `Result<PipelineControl, Err>`.

*   **`ConditionalScopeBuilder<'pipeline, TData, Err>`:** A fluent builder for defining conditional execution of scoped sub-pipelines within a step.

*   **Scoped Pipelines:** Independent `Pipeline<SData, Err>` instances (sharing the main pipeline's error type) executed conditionally, operating on an extracted sub-context `SData`.

*   **`PipelineProvider<TData, SData, MainErr>` (Trait):** Defines how scoped pipelines are sourced, statically or via an async factory.

*   **`Orka<ApplicationError>`:** A type-keyed registry for managing and running `Pipeline` instances. `ApplicationError` must be `From<OrkaError>`.

*   **`OrkaError` (Enum):** The framework's own error type. Application error types should be `From<OrkaError>`.

**Main Entry Points:**

*   `Pipeline::new(["step_a", "step_b"])`.
*   Configuring steps: `optional`, `required`, `skip_if`, `clear_skip_condition`, `insert_before_step`, `insert_after_step`, `remove_step`.
*   Registering handlers: `on_root`, `before_root`, `after_root`, and `on` for sub-contexts.
*   Sub-contexts: `set_extractor`, `set_extractor_with_merge`.
*   Conditional logic: `pipeline.conditional_scopes_for_step(..)`.
*   Checking setup: `pipeline.validate()`.
*   Registry: `Orka::new()`, `orka.register_pipeline(pipeline)?`, `orka.run(context_data).await`.

**Pervasive Types/Patterns:**

*   **`OrkaResult<T, E = OrkaError>` (Type Alias):** Orka's standard `Result`, defaulting to `OrkaError`.
*   **`ContextData<T>`:** Used for passing shared state to handlers.
*   **`PipelineControl` (Enum):** Returned by handlers to signal continue or stop.
*   **`PipelineResult` (Enum):** The outcome of a full run (`Completed` or `Stopped`).
*   **`From<OrkaError>` Trait Bound:** Required for application error types used with `Pipeline` or `Orka`.

**The `orka::prelude` module** re-exports the common surface: `Pipeline`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, and `Orka`. Advanced items (`Handler`, `ContextDataExtractorImpl`, the pipeline providers, and the conditional-scope builders) are imported from the crate root.

**Handler shape.** Handler futures resolve to `Result<PipelineControl, Err>`, where `Err` is the pipeline's own error type. Because `Err` is fixed by the pipeline, a bare `Ok(PipelineControl::Continue)` infers correctly and `?` converts other error types through `From`:

```rust
pipeline.on_root("load", |ctx| async move {
  let cfg = std::fs::read_to_string("cfg.toml")?; // converts via From<io::Error>
  ctx.write().config = cfg;
  Ok(PipelineControl::Continue)
});
```

## 2. Main Types and Their Public Methods

### Struct `orka::pipeline::definition::Pipeline<TData, Err>`

The core workflow definition.

**Generic Parameters:**

*   `TData: 'static + Send + Sync`
*   `Err: std::error::Error + From<OrkaError> + Send + Sync + 'static`

All configuration and registration methods return `&mut Self`, so calls chain.

#### Construction

*   **`pub fn new<I, S>(step_names: I) -> Self`**
    *   Where `I: IntoIterator<Item = S>`, `S: AsRef<str>`.
    *   Creates a pipeline from an ordered list of step names — `&["a", "b"]`, `["a", "b"]`, or `Vec<String>`.
    *   Every step starts **required** with no skip condition.
    *   **Panics** if the same step name appears twice.

#### Step configuration

*   **`pub fn optional(&mut self, step_name: &str) -> &mut Self`**
    *   Marks a step optional: it is skipped rather than failing when it has no handlers, and errors from its conditional scopes are swallowed. **Panics** if the step does not exist.

*   **`pub fn required(&mut self, step_name: &str) -> &mut Self`**
    *   Marks a step required (the default). **Panics** if the step does not exist.

*   **`pub fn skip_if(&mut self, step_name: &str, cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> &mut Self`**
    *   Sets a predicate that, when it returns `true` at run time, skips the step entirely (no `before`/`on`/`after` handlers run). **Panics** if the step does not exist.

*   **`pub fn clear_skip_condition(&mut self, step_name: &str) -> &mut Self`**
    *   Clears a skip condition previously set by `skip_if`. **Panics** if the step does not exist.

#### Step manipulation

*   **`pub fn insert_before_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S) -> &mut Self`**
    *   Inserts a new required step immediately before an existing one. Chain `optional` / `skip_if` afterwards to configure it. **Panics** if `existing_step_name` is unknown or `new_step_name` already exists.

*   **`pub fn insert_after_step<S: Into<String>>(&mut self, existing_step_name: &str, new_step_name: S) -> &mut Self`**
    *   Inserts a new required step immediately after an existing one. Same panics as above.

*   **`pub fn remove_step(&mut self, step_name: &str) -> &mut Self`**
    *   Removes a step and every handler, extractor, and conditional configuration registered against it. A no-op if the step is not found.

#### Root handlers

*   **`pub fn before_root<F>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
*   **`pub fn on_root<F>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
*   **`pub fn after_root<F>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
    *   Where `F: Future<Output = Result<PipelineControl, Err>> + Send + 'static`.
    *   Register a handler for the step's `before`, `on`, or `after` phase. Multiple handlers per phase are allowed and run in registration order. **Panics** if the step does not exist.

#### Sub-context handlers

*   **`pub fn set_extractor<SData>(&mut self, step_name: &str, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> &mut Self`**
    *   Where `SData: 'static + Send + Sync`.
    *   Registers an extractor producing a `ContextData<SData>` sub-context for the step. The sub-context is **detached**: writes made by the `on::<SData>` handler are not reflected in the root context. `ContextData::project` is the usual way to write one. **Panics** if the step does not exist.

*   **`pub fn set_extractor_with_merge<SData>(&mut self, step_name: &str, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static, merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static) -> &mut Self`**
    *   Where `SData: 'static + Send + Sync`.
    *   As `set_extractor`, but after the step's `on::<SData>` handler succeeds, `merge_fn` runs with a write lock on the root context and a read lock on the sub-context, folding the sub-pipeline's work into the parent. The merge runs **only when the handler returns `Ok`**. **Panics** if the step does not exist.

*   **`pub fn on<SData, F>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static) -> &mut Self`**
    *   Where `SData: 'static + Send + Sync`, `F: Future<Output = Result<PipelineControl, Err>> + Send + 'static`.
    *   Registers an `on`-phase handler operating on the step's extracted `ContextData<SData>`. Annotate the closure parameter (`|sub: ContextData<MyType>|`) to drive `SData` inference.
    *   **Panics** if the step does not exist, or if no extractor has been registered for it.
    *   Extractor and downcast failures are `OrkaError`s converted into `Err` via `From`.

    ```rust
    pipeline
      .set_extractor_with_merge(
        "validate",
        |main| Ok(main.project(|d| d.customer.clone())),
        |root, sub| root.customer = sub.clone(),
      )
      .on("validate", |sub: ContextData<Customer>| async move {
        sub.write().is_validated = true;
        Ok(PipelineControl::Continue)
      });
    ```

#### Conditional scopes

*   **`pub fn conditional_scopes_for_step(&mut self, step_name: &str) -> ConditionalScopeBuilder<'_, TData, Err>`**
    *   Prepares a step to host conditional scoped pipeline executions. The step is created if it does not already exist.
    *   The returned chain **must** be terminated with `finalize_conditional_step`, or the configured scopes are discarded; `validate` reports this if you forget.
    *   Finalizing **appends** the master handler, so `on_root` handlers already registered on the step still run.

#### Validation and execution

*   **`pub fn validate(&self) -> OrkaResult<()>`**
    *   Checks for setup mistakes that would otherwise surface at run time or silently do nothing:
        1.  A required step with no `before`/`on`/`after` handlers.
        2.  An extractor for a step with no `on::<SData>` handler consuming it.
        3.  A `conditional_scopes_for_step` builder that was never finalized.
    *   Collects **all** problems into a single `OrkaError::ConfigurationError`, not just the first. `Orka::register_pipeline` calls this for you.

*   **`pub async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>`**
    *   Executes the pipeline's steps sequentially. Framework errors during execution (for example a missing handler for a required step) are converted into `Err`.

### Struct `orka::core::context_data::ContextData<T>`

A wrapper for shared context data using `Arc<RwLock<T>>`.

**Generic Parameters:** `T: 'static + Send + Sync`

**Public Methods:**

*   **`pub fn new(data: T) -> Self`**
    *   Creates a new `ContextData` wrapping the given data.

*   **`pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, T>`**
    *   Acquires a read lock. The guard must be dropped before any `.await`.

*   **`pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, T>`**
    *   Acquires a write lock. The guard must be dropped before any `.await`.

*   **`pub fn try_read(&self) -> Option<parking_lot::RwLockReadGuard<'_, T>>`**
*   **`pub fn try_write(&self) -> Option<parking_lot::RwLockWriteGuard<'_, T>>`**
    *   Non-blocking lock attempts.

*   **`pub fn map_read<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockReadGuard<'_, U>`**
    *   Where `F: FnOnce(&T) -> &U`. Acquires a read lock mapped to part of the data.

*   **`pub fn map_write<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockWriteGuard<'_, U>`**
    *   Where `F: FnOnce(&mut T) -> &mut U`. Acquires a write lock mapped to part of the data.

*   **`pub fn project<U, F>(&self, get: F) -> ContextData<U>`**
    *   Where `U: Send + Sync + 'static`, `F: FnOnce(&T) -> U`.
    *   Builds a *new, independent* `ContextData<U>` from a projection of this one — the common shape of a sub-context extractor. The read guard is released before returning, so the result is safe to hold across an `.await`.
    *   The result does **not** share state with `self`; pair it with `set_extractor_with_merge` (or a scope's `with_merge`) to propagate writes back.

    ```rust
    pipeline.set_extractor("validate", |main| Ok(main.project(|d| d.customer.clone())));
    ```

**Implemented Traits:** `Clone`, `Debug`, `Default` (if `T: Default`).

### Struct `orka::conditional::builder::ConditionalScopeBuilder<'pipeline, TData, Err>`

Builder for defining conditional scopes within a pipeline step. Marked `#[must_use]`: scopes apply only when `finalize_conditional_step` is called.

**Generic Parameters:**

*   `'pipeline` (lifetime)
*   `TData: 'static + Send + Sync`
*   `Err: std::error::Error + From<OrkaError> + Send + Sync + 'static`

**Public Methods:**

*   **`pub fn add_static_scope<SData>(self, static_pipeline: Arc<Pipeline<SData, Err>>, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, StaticPipelineProvider<SData, Err>>`**
    *   Where `SData: 'static + Send + Sync`.
    *   Adds a scope backed by a pre-built `Pipeline<SData, Err>`.

*   **`pub fn add_dynamic_scope<SData, F, Fut>(self, pipeline_factory: F, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> ConditionalScopeConfigurator<'pipeline, TData, SData, Err, FunctionalPipelineProvider<TData, SData, Err, F, Fut>>`**
    *   Where `SData: 'static + Send + Sync`, `F: Fn(ContextData<TData>) -> Fut + Send + Sync + 'static`, `Fut: Future<Output = Result<Arc<Pipeline<SData, Err>>, OrkaError>> + Send + 'static`.
    *   Adds a scope whose sub-pipeline is produced per run by an async factory.

*   **`pub fn if_no_scope_matches(mut self, behavior: PipelineControl) -> Self`**
    *   Sets the control flow used when no scope's condition holds. Defaults to `PipelineControl::Continue`.

*   **`pub fn finalize_conditional_step(self, optional_for_main_step: bool)`**
    *   Registers the master handler on the step's `on` phase (appending, not replacing) and sets the step's optionality. When `optional_for_main_step` is `true`, an error from a matched scope is logged and the pipeline continues; otherwise it propagates.

### Struct `orka::conditional::builder::ConditionalScopeConfigurator<'pipeline, TData, SData, Err, P>`

Intermediate builder for a single conditional scope. Marked `#[must_use]`: the scope is registered only by `on_condition`.

**Generic Parameters:**

*   `'pipeline` (lifetime)
*   `TData: 'static + Send + Sync`
*   `SData: 'static + Send + Sync`
*   `Err: std::error::Error + From<OrkaError> + Send + Sync + 'static`
*   `P: PipelineProvider<TData, SData, Err> + 'static`

**Public Methods:**

*   **`pub fn with_merge(mut self, merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static) -> Self`**
    *   Folds this scope's context back into the main context after the scoped pipeline completes successfully. Without it the scope is detached and the main context sees nothing of the scoped pipeline's work. The merge runs **only when the scoped pipeline succeeds**.

    ```rust
    pipeline
      .conditional_scopes_for_step("pay")
      .add_static_scope(provider_a, |main| Ok(main.project(|d| d.payment.clone())))
      .with_merge(|main, sub| main.payment = sub.clone())
      .on_condition(|main| main.read().provider == Provider::A)
      .finalize_conditional_step(false);
    ```

*   **`pub fn on_condition(mut self, condition_fn: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> ConditionalScopeBuilder<'pipeline, TData, Err>`**
    *   Sets the predicate selecting this scope and returns the builder, so further scopes can be added or the step finalized. Scopes are tested in registration order; the first match wins.

### Struct `orka::registry::Orka<ApplicationError = OrkaError>`

A type-keyed registry for managing and executing `Pipeline` instances.

**Generic Parameters:**

*   `ApplicationError: std::error::Error + From<OrkaError> + Send + Sync + 'static` (defaults to `OrkaError`)

**Public Methods:**

*   **`pub fn new() -> Self`**
    *   Creates a new, empty registry.

*   **`pub fn new_default() -> Orka<OrkaError>`**
    *   Convenience constructor for a registry using `OrkaError` as its application error type.

*   **`pub fn register_pipeline<TData, PipelineHandlerError>(&self, pipeline: Pipeline<TData, PipelineHandlerError>) -> OrkaResult<()>`**
    *   Where:
        *   `TData: 'static + Send + Sync`
        *   `PipelineHandlerError: std::error::Error + From<OrkaError> + Send + Sync + 'static`
        *   `ApplicationError: From<PipelineHandlerError>`
        *   `Pipeline<TData, PipelineHandlerError>: Send + Sync`
    *   Validates the pipeline via `Pipeline::validate`, then registers it keyed by the `TypeId` of `TData`. Registering a second pipeline for the same `TData` replaces the first.
    *   **Errors:** returns `OrkaError::ConfigurationError` if validation fails.

*   **`pub async fn run<TData>(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, ApplicationError>`**
    *   Where `TData: 'static + Send + Sync`.
    *   Looks up the pipeline registered for `TData` and executes it. Returns `OrkaError::ConfigurationError` (converted to `ApplicationError`) if no pipeline is registered for that type.

## 3. Public Traits and Their Methods

### Trait `orka::conditional::provider::PipelineProvider<TData, SData, MainErr>`

Defines a contract for objects that can provide scoped pipeline instances.

**Generic Parameters:**

*   `TData: 'static + Send + Sync`
*   `SData: 'static + Send + Sync`
*   `MainErr: std::error::Error + From<OrkaError> + Send + Sync + 'static`

**Methods:**

*   **`async fn get_pipeline(&self, main_ctx_data: ContextData<TData>) -> Result<Arc<Pipeline<SData, MainErr>>, OrkaError>`**
    *   Gets or creates an `Arc<Pipeline<SData, MainErr>>`. The provider's own operation can fail with an `OrkaError`.

**Implementors:**

*   `StaticPipelineProvider<SData, Err>` — yields a pre-built pipeline.
*   `FunctionalPipelineProvider<TData, SData, Err, F, Fut>` — yields a pipeline from an async factory.

Both are re-exported at the crate root.

## 4. Public Enums (Non-Config)

### Enum `orka::core::control::PipelineControl`

Signal from a handler indicating whether the pipeline should continue or stop.

**Variants:**

*   **`Continue`**: Proceed with the current step and subsequent steps.
*   **`Stop`**: Halt immediately — no further handlers in the current step, no further steps.

### Enum `orka::core::control::PipelineResult`

Outcome of a full pipeline execution.

**Variants:**

*   **`Completed`**: All non-skipped steps ran to completion.
*   **`Stopped`**: A handler returned `PipelineControl::Stop`.

## 5. Public Type Aliases

### `pub type Handler<TData, Err> = Box<dyn Fn(ContextData<TData>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PipelineControl, Err>> + Send>> + Send + Sync>`
*   (Located in `orka::core::context`)
*   The stored form of a step handler. Registration methods build this for you from a plain `async` closure.

### `pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>`
*   (Located in `orka::error`)
*   A result type defaulting to `OrkaError`. Returned by `Pipeline::validate` and `Orka::register_pipeline`.

### `pub type SkipCondition<TData> = std::sync::Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>`
*   (Located in `orka::core::step`, re-exported at the crate root)
*   The stored form of a step's skip predicate. `Pipeline::skip_if` builds this from a plain closure.

## 6. Error Handling

### Enum `orka::error::OrkaError`

The framework's error type.

**Variants:**

*   **`StepNotFound { step_name: String }`**
*   **`HandlerMissing { step_name: String }`**: A required step lacks handlers.
*   **`ExtractorFailure { step_name: String, source: anyhow::Error }`**
*   **`PipelineProviderFailure { step_name: String, source: anyhow::Error }`**
*   **`TypeMismatch { step_name: String, expected_type: String }`**
*   **`HandlerError { source: anyhow::Error }`**: Wraps an error from user handler logic or an external operation converted into `OrkaError`.
*   **`ConfigurationError { step_name: String, message: String }`**: Validation failures and registry lookup misses. `Pipeline::validate` reports all problems in one instance of this variant.
*   **`Internal(String)`**
*   **`NoConditionalScopeMatched { step_name: String }`**

**Conversions:**

*   **`impl From<anyhow::Error> for OrkaError`** — wraps as `HandlerError`. An `anyhow::Error` already wrapping an `OrkaError` stays nested; the causal chain is preserved via `#[source]`.

**Standard Result Type:**

*   **`pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;`**
    *   Applications typically define their own error enum with `#[from] OrkaError`, letting framework errors convert into the application's error domain.

### Panics versus errors

Setup mistakes that are unambiguous programming errors **panic** at the offending call: an unknown step name, a duplicate step name, or `on::<SData>` registered without an extractor. Problems only visible once the pipeline is assembled are reported by `Pipeline::validate` as `OrkaError::ConfigurationError`.

## 7. Modules

The public API is primarily exposed through re-exports in `orka::lib.rs`.

*   **`orka::prelude`**: The common imports — `Pipeline`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, `Orka`.
*   **`orka::pipeline`**: The `Pipeline` definition.
*   **`orka::core`**: `ContextData`, `Handler`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`.
*   **`orka::conditional`**: `ConditionalScopeBuilder`, `ConditionalScopeConfigurator`, the `PipelineProvider` trait, and its implementors.
*   **`orka::registry`**: The `Orka` registry.
*   **`orka::error`**: `OrkaError` and `OrkaResult`.
