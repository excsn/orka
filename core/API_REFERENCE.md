# Orka Workflow Engine (0.3+) - API Reference

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
*   Configuring steps: `optional`, `required`, `skip_if`, `skip_if_labeled`, `clear_skip_condition`, `must_precede`, `must_precede_all`, `produces`, `consumed_by`, `insert_before_step`, `insert_after_step`, `remove_step`.
*   Registering handlers: `on_root`, `before_root`, `after_root`, and `on` for sub-contexts.
*   Sub-contexts: `set_extractor`, `set_extractor_with_merge`.
*   Conditional logic: `pipeline.conditional_scopes_for_step(..)`.
*   Checking setup: `pipeline.validate()`.
*   Registry: `Orka::new()`, `orka.register_pipeline(pipeline)?`, `orka.run(context_data).await`.

**Pervasive Types/Patterns:**

*   **`OrkaResult<T, E = OrkaError>` (Type Alias):** Orka's standard `Result`, defaulting to `OrkaError`.
*   **`ContextData<T>`:** Used for passing shared state to handlers.
*   **`PipelineControl` (Enum):** Returned by handlers to signal continue or stop.
*   **`PipelineResult` (Enum):** The outcome of a full run (`Completed`, `Stopped`, or `Cancelled`).
*   **`CancelToken`:** Out-of-band cancellation, for anything that is not the running handler. Section 10.
*   **`From<OrkaError>` Trait Bound:** Required for application error types used with `Pipeline` or `Orka`.
*   **Step names (`impl AsRef<str>`):** Every parameter naming a step accepts anything `AsRef<str>`: a `&str` literal, a `String`, or a typed key. Giving a step enum an `AsRef<str>` impl makes step references typo-proof, rename-refactorable, and autocompleting; typed keys and strings mix freely.

**The `orka::prelude` module** re-exports the everyday surface: `Pipeline`, `PipelineRunner`, `Orka`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, and the types that appear in `Pipeline`'s own signatures (`RunOutcome`, `StepPlan`, `PlannedAction`, `SkipReason`, `StepPhase`, `CancelToken`, `Cancelled`). Advanced items (`Handler`, `ContextDataExtractorImpl`, `AnyContextDataExtractor`, the pipeline providers, the conditional-scope builders) and the observability surface (`TraceCollector`, `PipelineObserver`, `CompositeObserver`, `TraceEvent`, `TraceEventKind`, `HandlerOutcome`, `RunTrace`) are imported from the crate root.

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
    *   Creates a pipeline from an ordered list of step names: `&["a", "b"]`, `["a", "b"]`, or `Vec<String>`.
    *   Every step starts **required** with no skip condition.
    *   **Panics** if the same step name appears twice.

#### Step configuration

*   **`pub fn optional(&mut self, step_name: impl AsRef<str>) -> &mut Self`**
    *   Marks a step optional: it is skipped rather than failing when it has no handlers, and errors from its conditional scopes are swallowed. **Panics** if the step does not exist.

*   **`pub fn required(&mut self, step_name: impl AsRef<str>) -> &mut Self`**
    *   Marks a step required (the default). **Panics** if the step does not exist.

*   **`pub fn skip_if(&mut self, step_name: impl AsRef<str>, cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> &mut Self`**
    *   Sets a predicate that, when it returns `true` at run time, skips the step entirely (no `before`/`on`/`after` handlers run). Clears any label a previous `skip_if_labeled` set. **Panics** if the step does not exist.

*   **`pub fn skip_if_labeled(&mut self, step_name: impl AsRef<str>, label: impl Into<String>, cond: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> &mut Self`**
    *   As `skip_if`, with a human-readable label ("drain disabled by config") carried into `resolve_plan` output and `StepSkipped` trace events via `SkipReason::SkipCondition { label }`. **Panics** if the step does not exist.

*   **`pub fn clear_skip_condition(&mut self, step_name: impl AsRef<str>) -> &mut Self`**
    *   Clears a skip condition (and its label) previously set by `skip_if` / `skip_if_labeled`. **Panics** if the step does not exist.

*   **`pub fn must_precede(&mut self, before: impl AsRef<str>, after: impl AsRef<str>) -> &mut Self`**
    *   Declares an ordering invariant checked by `validate` (not at run time): `before` must appear earlier in the step order than `after`. `remove_step` deliberately does **not** clean these constraints; a dangling constraint fails `validate`, catching the removal of a step others depend on. **Panics** if either step does not exist at declaration, or if `before == after`.

*   **`pub fn must_precede_all<I, S>(&mut self, before: impl AsRef<str>, afters: I) -> &mut Self`**
    *   Where `I: IntoIterator<Item = S>`, `S: AsRef<str>`. One `must_precede` per target. **Panics** on an unknown step or if `before` appears among `afters`.

#### Resource dependencies

*   **`pub fn produces(&mut self, step_name: impl AsRef<str>, resource: impl AsRef<str>) -> &mut Self`**
    *   Declares that a step produces a named resource (a value later steps read out of the context). **Panics** if the step does not exist.

*   **`pub fn consumed_by<I, S>(&mut self, resource: impl AsRef<str>, steps: I) -> &mut Self`**
    *   Where `I: IntoIterator<Item = S>`, `S: AsRef<str>`. Declares the steps that read a resource. **Panics** if any step does not exist.
    *   Together these say what a chain of `must_precede` pairs can only imply. `validate` derives the ordering (every producer must precede every consumer, since which producer runs is not knowable statically) and additionally reports **a resource that is consumed but that no step produces**, the shape a renamed or deleted producer takes before it becomes a mid-run `.expect()` panic. That is reported once per resource, naming every affected consumer. Like `must_precede`, these declarations are not cleaned by `remove_step`; a dangling one fails `validate`. Resource names are `AsRef<str>`, so a typed `Res` enum works.

#### Step manipulation

*   **`pub fn insert_before_step(&mut self, existing_step_name: impl AsRef<str>, new_step_name: impl AsRef<str>) -> &mut Self`**
    *   Inserts a new required step immediately before an existing one. Chain `optional` / `skip_if` afterwards to configure it. **Panics** if `existing_step_name` is unknown or `new_step_name` already exists.

*   **`pub fn insert_after_step(&mut self, existing_step_name: impl AsRef<str>, new_step_name: impl AsRef<str>) -> &mut Self`**
    *   Inserts a new required step immediately after an existing one. Same panics as above.

*   **`pub fn remove_step(&mut self, step_name: impl AsRef<str>) -> &mut Self`**
    *   Removes a step and every handler, extractor, and conditional configuration registered against it. A no-op if the step is not found.

#### Root handlers

*   **`pub fn before_root<F>(&mut self, step_name: impl AsRef<str>, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
*   **`pub fn on_root<F>(&mut self, step_name: impl AsRef<str>, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
*   **`pub fn after_root<F>(&mut self, step_name: impl AsRef<str>, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static) -> &mut Self`**
    *   Where `F: Future<Output = Result<PipelineControl, Err>> + Send + 'static`.
    *   Register a handler for the step's `before`, `on`, or `after` phase. Multiple handlers per phase are allowed and run in registration order. **Panics** if the step does not exist.

#### Sub-context handlers

*   **`pub fn set_extractor<SData>(&mut self, step_name: impl AsRef<str>, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> &mut Self`**
    *   Where `SData: 'static + Send + Sync`.
    *   Registers an extractor producing a `ContextData<SData>` sub-context for the step. The sub-context is **detached**: writes made by the `on::<SData>` handler are not reflected in the root context. `ContextData::project` is the usual way to write one. **Panics** if the step does not exist.

*   **`pub fn set_extractor_with_merge<SData>(&mut self, step_name: impl AsRef<str>, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static, merge_fn: impl Fn(&mut TData, &SData) + Send + Sync + 'static) -> &mut Self`**
    *   Where `SData: 'static + Send + Sync`.
    *   As `set_extractor`, but after the step's `on::<SData>` handler succeeds, `merge_fn` runs with a write lock on the root context and a read lock on the sub-context, folding the sub-pipeline's work into the parent. The merge runs **only when the handler returns `Ok`**. **Panics** if the step does not exist.

*   **`pub fn on<SData, F>(&mut self, step_name: impl AsRef<str>, handler_fn: impl Fn(ContextData<SData>) -> F + Send + Sync + 'static) -> &mut Self`**
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

*   **`pub fn conditional_scopes_for_step(&mut self, step_name: impl AsRef<str>) -> ConditionalScopeBuilder<'_, TData, Err>`**
    *   Prepares a step to host conditional scoped pipeline executions. The step is created if it does not already exist.
    *   The returned chain **must** be terminated with `finalize_conditional_step`, or the configured scopes are discarded; `validate` reports this if you forget.
    *   Finalizing **appends** the master handler, so `on_root` handlers already registered on the step still run.

#### Validation and execution

*   **`pub fn validate(&self) -> OrkaResult<()>`**
    *   Checks for setup mistakes that would otherwise surface at run time or silently do nothing:
        1.  A required step with no `before`/`on`/`after` handlers.
        2.  An extractor for a step with no `on::<SData>` handler consuming it.
        3.  A `conditional_scopes_for_step` builder that was never finalized.
        4.  A `must_precede` ordering constraint violated by the actual step order, or referencing a step no longer in the pipeline.
        5.  A `produces` / `consumed_by` declaration referencing a step no longer in the pipeline.
        6.  A resource that is consumed but that no step produces.
        7.  A resource consumed by a step that runs earlier than the step producing it.
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

*   **`pub fn resources(&self) -> &RunResources`**
    *   The run-scoped resource bag shared by every handle to this context. See `RunResources` below.

*   **`pub fn cancellation(&self) -> CancelToken`**
    *   The run's cancellation token, shared by every handle to this context. Always present, so a handler never branches on whether the run is cancellable. See §10.

*   **`pub fn require<R, F>(&self, resource: impl AsRef<str>, get: F) -> Result<R, OrkaError>`**
    *   Where `F: FnOnce(&T) -> Option<R>`. Reads a value a previous step should have produced, failing with `OrkaError::ResourceMissing` rather than panicking. The runtime counterpart to `produces` / `consumed_by`: `ctx.require(Res::AppSpec, |c| c.app_spec.clone())?` replaces `.expect("app_spec set by load-spec step")`.
    *   The gain is cleanup, not tidiness: a panic unwinds past the `on_finish` ring and past `RunResources` release, and what does drop then drops front to back rather than in reverse; inside a spawned fan-out it degrades further into `FanOutBranchLost`. It does **not** check the name against the declarations (a context does not know which step is reading it); what couples them is using the same key in both places. The step comes from `run_with_outcome`.

*   **`pub fn with_ref<F, R>(&self, f: F) -> R`**
    *   Where `F: FnOnce(&T) -> R`. Runs `f` under a read lock and releases the guard before returning its result.

*   **`pub fn with_mut<F, R>(&self, f: F) -> R`**
    *   Where `F: FnOnce(&mut T) -> R`. The mutating counterpart.
    *   These two make the "no guard across `.await`" rule structural: `f` is synchronous and the guard's scope is the call, so the lock cannot reach a suspension point. Prefer them to `read`/`write` in handler bodies. Because they return the closure's value, a non-`Clone` intermediate can be mutated in place and its ownership taken back out (`ctx.with_mut(|c| c.specs.take())`) with no `Arc` and no `try_unwrap` dance. A field that is `Arc<U>` because something outside the context shares it remains `Arc::get_mut`'s problem, not theirs.

*   **`pub fn map_read<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockReadGuard<'_, U>`**
    *   Where `F: FnOnce(&T) -> &U`. Acquires a read lock mapped to part of the data. Borrows a field without cloning it, which is often why an inner `Arc` turns out to be unnecessary.

*   **`pub fn map_write<F, U: ?Sized>(&self, f: F) -> parking_lot::MappedRwLockWriteGuard<'_, U>`**
    *   Where `F: FnOnce(&mut T) -> &mut U`. Acquires a write lock mapped to part of the data.

*   **`pub fn project<U, F>(&self, get: F) -> ContextData<U>`**
    *   Where `U: Send + Sync + 'static`, `F: FnOnce(&T) -> U`.
    *   Builds a *new, independent* `ContextData<U>` from a projection of this one, the common shape of a sub-context extractor. The read guard is released before returning, so the result is safe to hold across an `.await`.
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

*   `StaticPipelineProvider<SData, Err>`: yields a pre-built pipeline.
*   `FunctionalPipelineProvider<TData, SData, Err, F, Fut>`: yields a pipeline from an async factory.

Both are re-exported at the crate root.

## 4. Public Enums (Non-Config)

### Enum `orka::core::control::PipelineControl`

Signal from a handler indicating whether the pipeline should continue or stop.

**Variants:**

*   **`Continue`**: Proceed with the current step and subsequent steps.
*   **`Stop`**: Halt immediately, with no further handlers in the current step and no further steps.

### Enum `orka::core::control::PipelineResult`

Outcome of a full pipeline execution.

Marked `#[non_exhaustive]`: match it with a wildcard arm.

**Variants:**

*   **`Completed`**: All non-skipped steps ran to completion.
*   **`Stopped`**: A handler returned `PipelineControl::Stop`.
*   **`Cancelled`**: The run reached a step boundary with its `CancelToken` set. See section 10.

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
*   **`FanOutPolicyUnmet { policy: String, total: usize, succeeded: usize, failed: usize, not_started: usize }`**: A fan-out finished without satisfying its policy and no branch produced an error to propagate. See section 9.
*   **`FanOutBranchLost { index: usize }`**: A spawned fan-out branch never produced a result, meaning its task panicked or was aborted. Only reachable with a `TaskSpawner` configured.
*   **`ResourceMissing { resource: String }`**: A step read a resource that is not in the context, via `ContextData::require`. Exists so a missing resource is a handled error instead of an `.expect()` panic that would unwind past the finish ring and resource release.
*   **`StepTimedOut { step_name: String, after: std::time::Duration }`**: A step exceeded its time budget. Orka imposes no timeouts and owns no timer, since it depends on no runtime; handlers bound their own work and return this so the failure is reported uniformly and carries the step name into `run_with_outcome` and the trace. Note that a timeout drops the handler mid-flight, abandoning its partial work; the run still reaches its `on_finish` ring and releases its resource bag.

**Conversions:**

*   **`impl From<anyhow::Error> for OrkaError`**: wraps as `HandlerError`. An `anyhow::Error` already wrapping an `OrkaError` stays nested; the causal chain is preserved via `#[source]`.

**Standard Result Type:**

*   **`pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;`**
    *   Applications typically define their own error enum with `#[from] OrkaError`, letting framework errors convert into the application's error domain.

### Panics versus errors

Setup mistakes that are unambiguous programming errors **panic** at the offending call: an unknown step name, a duplicate step name, or `on::<SData>` registered without an extractor. Problems only visible once the pipeline is assembled are reported by `Pipeline::validate` as `OrkaError::ConfigurationError`.

## 7. Modules

The public API is primarily exposed through re-exports in `orka::lib.rs`.

*   **`orka::prelude`**: The everyday imports: `Pipeline`, `PipelineRunner`, `Orka`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, `RunOutcome`, `StepPlan`, `PlannedAction`, `SkipReason`, `StepPhase`, `CancelToken`, `Cancelled`.
*   **`orka::pipeline`**: The `Pipeline` definition.
*   **`orka::core`**: `ContextData`, `Handler`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `CancelToken`, `Cancelled`.
*   **`orka::conditional`**: `ConditionalScopeBuilder`, `ConditionalScopeConfigurator`, the `PipelineProvider` trait, and its implementors.
*   **`orka::registry`**: The `Orka` registry.
*   **`orka::error`**: `OrkaError` and `OrkaResult`.

## 8. Testing and Observability (added in 0.3)

All of this section is additive. Everything except `orka::test_util` is unconditional API; `test_util` requires the `test-util` cargo feature (enable it in `[dev-dependencies]`).

### Run-level finish handlers (`Pipeline`)

*   **`pub fn on_finish<F>(&mut self, handler_fn: impl Fn(ContextData<TData>, RunOutcome) -> F + Send + Sync + 'static) -> &mut Self`** where `F: Future<Output = Result<(), Err>> + Send + 'static`
    *   Registers an async "finally" awaited on **every exit of a full `run()`** (Completed, Stopped, Cancelled, or Errored, including the missing-handler configuration error), with the final shared context and the run's `RunOutcome`. Handlers run in registration order and all of them run even if one fails. On an `Ok` run the first finish-handler error becomes the run's error; on a failed run finish-handler errors are logged and the original error is preserved. Partial runners and `resolve_plan` never fire finish handlers.
*   **`pub fn clear_finish_handlers(&mut self) -> &mut Self`**

### Observation (`Pipeline`, `&self` attachment)

*   **`pub fn set_observer(&self, observer: Arc<dyn PipelineObserver>) -> &Self`** / **`pub fn set_tracer(&self, tracer: TraceCollector) -> &Self`** / **`pub fn clear_observer(&self) -> &Self`**
    *   At most one observer is attached; attaching replaces. `&self`: works on a pipeline behind an `Arc` (for example from `Orka::pipeline`). The observer is snapshotted at run start, so attaching mid-run catches the next run.
*   **Trait `orka::PipelineObserver`**: `fn on_event(&self, event: &TraceEvent)` (synchronous, must not block) and `fn on_handler_error(&self, run_id: u64, step: &str, phase: StepPhase, error: &(dyn std::error::Error + 'static))` (default no-op; the only place the concrete handler error is reachable, via `downcast_ref`).
*   **`TraceEvent { run_id: u64, kind: TraceEventKind }`**; `TraceEventKind`: `RunStarted`, `StepStarted`, `StepSkipped { reason: SkipReason }`, `HandlerFinished { phase: StepPhase, handler_index, outcome: HandlerOutcome }`, `StepCompleted`, `ScopeMatched { scope_index }`, `ScopeNotMatched`, `FinalizerFinished`, `RunCancelled { step, index }` (§10), `ResourcesReleased { count }`, `RunFinished { outcome: RunOutcome }`. Supporting enums: `StepPhase { Before, On, After }`, `SkipReason { SkipCondition { label: Option<String> }, OptionalWithoutHandlers }` (`label` from `skip_if_labeled`; `Display` renders the label when present), `HandlerOutcome { Continue, Stop, Error(String) }`, `RunOutcome { Completed, Stopped, Cancelled, Errored { step, message } }` (`#[non_exhaustive]`). All `Clone + PartialEq + Display`; `Serialize` with the `serde` feature.
*   **`TraceCollector`** (Clone, Default): buffering `PipelineObserver`. `new/record/events/clear`, `run_ids()`, `for_run(run_id) -> RunTrace` (same queries scoped to one run), `completed_steps()`, `skipped_steps()`, `step_completed(&str)`, `step_skipped(&str)`, `handler_finishes(&str, StepPhase)`, `run_count()`, `last_outcome()`. An accumulating log; flat queries are only unambiguous for a single recorded run.
*   **`CompositeObserver`** (Clone, Default): fans out to multiple observers, in order. `new()`, `with(Vec<Arc<dyn PipelineObserver>>)`, `push(..)`. The pipeline slot holds one observer by design; compose a production bridge and a diagnostic collector with this.

### Dry-run and partial execution (`Pipeline`)

*   **`pub fn resolve_plan(&self, ctx_data: &ContextData<TData>) -> Vec<StepPlan>`** with `StepPlan { name: String, action: PlannedAction }`, `PlannedAction { Run, Skip(SkipReason), FailMissingHandlers, Cancelled }` (`#[non_exhaustive]`; `Cancelled` when the context's token is already set, see §10). Both implement `Display`, so a plan prints as a preview (`"drain: skip (drain disabled by config)"`).
    *   Evaluates skip predicates and handler-presence checks against a seeded context, executing nothing. Static preview: step-to-step data flow is not simulated.
*   **`pub async fn run_step(&self, step_name: impl AsRef<str>, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>`** / **`run_from`** / **`run_until`**
    *   Execute one step / the named step through the end / the start through the named step (inclusive). `skip_if` is respected. Inspection tools: no `RunStarted`/`RunFinished` events, no finish handlers. Unknown step: `OrkaError::StepNotFound` via `Err::from`.

### Run-scoped resources

**Struct `orka::RunResources`** holds RAII values a run must *hold* rather than *operate on* (lock guards, temp dirs, file handles), so they need not ride through the context struct as `Option<T>` data fields. Reached via `ContextData::resources()`.

*   **`pub fn put<R: Send + 'static>(&self, resource: R) -> &Self`**
    *   Stashes a resource; returns `&Self` so several chain (`ctx.resources().put(guard).put(temp)`).
*   **`pub fn with<R: Send + 'static, F, T>(&self, f: F) -> Option<T>`**
    *   Where `F: FnOnce(&R) -> T`. Borrows the most recently stashed value of type `R`, `None` if none is held. The bag's lock is held while `f` runs, so `f` must not touch the same bag or block.
*   **`pub fn take<R: Send + 'static>(&self) -> Option<R>`**
    *   Removes the most recently stashed resource of that type and hands over ownership. For genuine consumption only: between a `take` and a manual `put` the value lives in a local, so an early `?`, a timeout, or a panic drops it there instead of at the run's release point.
*   **`pub fn take_guard<R: Send + 'static>(&self) -> Option<TakenResource<'_, R>>`**
    *   Takes the resource out on loan, returning it to the bag when the guard drops. `with` holds the bag's lock for its closure, so a resource borrowed that way cannot cross an `.await`; this is for resources a run *operates on* rather than merely holds, such as a stream sender that chunks are awaited into. `TakenResource` derefs to the value, is an ordinary owned guard rather than a lock guard (so holding it across awaits is the point), and offers `keep(self) -> R` to opt out of the return. Because the return happens in `Drop`, no path skips it: an early `?`, a panic, or a handler cancelled by a timeout all put the resource back, so it is still released at the run's defined point.
*   **`pub fn len(&self) -> usize`** / **`pub fn is_empty(&self) -> bool`**

Lifecycle: a full `Pipeline::run` releases everything in **reverse** insertion order, **after** its `on_finish` handlers (so a finalizer can still use a temp dir or lock) and unconditionally (a failed run releases like a successful one). The release emits `TraceEventKind::ResourcesReleased { count }`, but only when the bag was non-empty. The partial runners and `resolve_plan` leave the bag alone, matching the finish-ring rule; whatever remains drops when the last `ContextData` handle drops. There is no async `Drop` in Rust, so cleanup that must be awaited belongs in `on_finish` instead. `ContextData::project` starts its independent context with an empty bag.

### Failed-step identity

*   **`Pipeline::run_with_outcome(&self, ctx_data: ContextData<TData>) -> (Result<PipelineResult, Err>, RunOutcome)`**: as `run()` (which wraps it), additionally returning the `RunOutcome`; on failure `Errored { step, message }` names the failing step (`"on_finish"` when a finish handler failed an otherwise-Ok run).
*   **`PipelineRunner::run_with_outcome`**: defaulted trait method; the default derives the outcome from `run()` with an empty `step` on failure (the runner cannot attribute it). `Pipeline` overrides with real attribution; middleware should delegate to its inner runner's `run_with_outcome`.
*   **`Orka::run_with_outcome<TData>(&self, ctx_data) -> (Result<PipelineResult, ApplicationError>, RunOutcome)`**: registry passthrough; a missing registration yields `Errored { step: "Orka::run", .. }`.
*   **`Pipeline::run_with_cancel`**, **`run_with_cancel_and_outcome`**, **`run_with_observer_and_cancel`**, and **`Orka::run_with_cancel`** / **`run_with_cancel_and_outcome`**: the same entry points taking a `CancelToken`. See §10.

### Per-run observers

*   **`Pipeline::run_with_observer(&self, ctx_data, observer: Arc<dyn PipelineObserver>) -> (Result<PipelineResult, Err>, RunOutcome)`**
    *   As `run_with_outcome`, with an observer scoped to **this call** rather than attached to the pipeline. `set_observer` binds to the pipeline, so a registered pipeline shared by concurrent runs reports all of them into one collector, and you cannot filter to your own: the run id is allocated inside the run, so there is nothing to pass to `for_run`.
    *   A pipeline-attached observer is **not** displaced; both receive every event.
    *   The scoped observer is **inherited by nested runs** (fan-out branches, conditional sub-pipelines), so one collector sees the whole call tree. Inheritance is deliberately limited to scoped observers; an attached one stays bound to its own pipeline's runs, so existing behaviour is unchanged. Inheritance gives *isolation* ("these events are mine"), not *hierarchy*: branch runs carry their own run ids and nothing yet records which parent step spawned them, which would need the deferred `FanOutStarted`/`FanOutFinished` events.

### Introspection and overrides (`Pipeline`)

*   **`pub fn step_names(&self) -> Vec<String>`**, **`pub fn has_handlers(&self, step_name: impl AsRef<str>, phase: StepPhase) -> bool`**
*   **`clear_before` / `clear_on` / `clear_after` (`&mut self, step_name: impl AsRef<str>) -> &mut Self`**: empty one phase's handler vec; surgical. `clear_on` leaves the extractor in place; an orphaned extractor then fails `validate`, deliberately.
*   **`replace_before_root` / `replace_on_root` / `replace_after_root`**: clear that phase, then register (same signature as the `*_root` registrations). Replace-all per (step, phase); handlers have no identity, so per-handler targeting does not exist.
*   **`pub fn remove_extractor(&mut self, step_name: impl AsRef<str>) -> &mut Self`**
*   **`pub fn stub_step(&mut self, step_name: impl AsRef<str>) -> &mut Self`**: neutralizes a whole step (all phases, extractor, any conditional master handler) and installs a single Continue `on` handler so `validate` passes. Finish handlers are untouched.

### Run boundary and registry

*   **Trait `orka::PipelineRunner<TData, Err>`** (async, object-safe): `async fn run(&self, ctx_data: ContextData<TData>) -> Result<PipelineResult, Err>`. Implemented by `Pipeline`. The composition seam at the run boundary: mocks in tests, retry/timeout/logging middleware in production.
*   **`Orka::register_runner<TData, PipelineHandlerError>(&self, runner: Arc<dyn PipelineRunner<TData, PipelineHandlerError>>)`**: registers any runner under `TData`, no validation, replacing any prior registration.
*   **`Orka::pipeline<TData, PipelineHandlerError>(&self) -> Option<Arc<Pipeline<TData, PipelineHandlerError>>>`**: the registered concrete pipeline, or `None` for runner-only registrations and mismatched `PipelineHandlerError`. Everything `&self` on the pipeline (observers, `resolve_plan`, partial runners, introspection) is reachable through it; `&mut` overrides are deliberately not (build, mutate, then register into a fresh registry instead).

### Injection seams

*   **`Pipeline::set_extractor_impl(&mut self, step_name: impl AsRef<str>, extractor: Arc<dyn AnyContextDataExtractor<TData>>) -> &mut Self`**: the seam behind `set_extractor`/`set_extractor_with_merge`. `on::<SData>` captures the extractor at registration time, so install the extractor before registering consumers. `AnyContextDataExtractor` is re-exported at the crate root.
*   **`ConditionalScopeBuilder::add_scope_with_provider<SData>(self, provider: Arc<dyn PipelineProvider<TData, SData, Err>>, extractor_fn) -> ConditionalScopeConfigurator<..., DynPipelineProvider<TData, SData, Err>>`**: the trait-object generalization of `add_static_scope`/`add_dynamic_scope`. `DynPipelineProvider` is the public adapter type.

### Module `orka::test_util` (feature `test-util`)

*   **`TestError`**: `Clone + PartialEq` pipeline error; `From<OrkaError>` stringifies. Variants: `Orka(String)`, `Handler(String)`, `Extractor(String)`, `Provider(String)`, `ScopedTask(String)`, `Other(String)`.
*   **`ExecutionCounter`** (Clone): `new/increment/get/reset`; the local replacement for global atomic counters and `#[serial]`.
*   **Handler factories**: `continue_handler()`, `stop_handler()`, `fail_handler(make_err)`, `counting_handler(counter)`; return closures compatible with the `*_root`/`replace_*_root` registrations.
*   **Trait `PipelineTestExt`**: `fail_at(&mut self, step_name, make_err) -> &mut Self` on `Pipeline` (replace `on` with a failing handler).
*   **`noop_pipeline(step_names) -> Pipeline<TData, Err>`**: continue-only pipeline over the given step names.
*   **`MockPipeline<TData, Err>`** (implements `PipelineRunner`): `completed()`, `stopped()`, `failing(make_err)`, `from_fn(f)`; FIFO one-shot queue `then_completed()/then_stopped()/then_error(make_err)`; inspection `run_count()`, `contexts() -> Vec<ContextData<TData>>`.
*   **Trace assertions** (all `#[track_caller]`): `assert_steps_completed(&TraceCollector, &[&str])`, `assert_steps_skipped(..)`, `assert_run_outcome(&TraceCollector, RunOutcome)`, `assert_order(..)` (in-order subsequence of completed steps).

### Cargo features

*   **`test-util`**: ships `orka::test_util`. Consumers enable it in `[dev-dependencies]`; orka's own canonical invocation is `cargo test --features test-util` (a path-only self dev-dependency makes bare `cargo test` work too and is stripped on publish).
*   **`serde`**: `Serialize` on the trace and plan types, for snapshot-style assertions.
*   **`tokio`**: ships `TokioSpawner` for fan-out and `timed` for bounding one await inside a handler. Off by default, and the only feature that adds a runtime dependency (tokio with just `rt` and `time`).

### Timeouts (feature `tokio`)

*   **`pub async fn orka::timed<T, F>(step_name: impl AsRef<str>, budget: Duration, fut: F) -> Result<T, OrkaError>`** where `F: Future<Output = T>`
    *   Awaits `fut` with a budget, reporting an overrun as `OrkaError::StepTimedOut { step_name, after }` instead of an anonymous elapsed error. The returned `OrkaError` converts into the pipeline's error via the usual `From` bound, so one `?` discharges it; a future that itself yields a `Result` needs a second `?` for its own failure mode.
    *   Bounds **that await only**, not the whole handler, so a timeout stays something you can react to rather than only fail on. Orka has no timer of its own and imposes no timeouts; this is the reporting, not an engine feature.
    *   On expiry `fut` is dropped and its in-flight work is abandoned. The run continues to its exit, so its `on_finish` ring and resource bag still fire; only state the future held locally is lost, which is a reason to stash anything needing orderly shutdown in `ContextData::resources()`.

## 9. Fan-out (all-of-N)

Runs one pipeline over every item of a runtime collection. The counterpart to `conditional_scopes_for_step` (one-of-N). A combinator called from inside a handler, not a step builder. All items are re-exported at the crate root and deliberately **not** in the prelude.

### Struct `orka::FanOut<SData, Err>`

*   **`pub fn new(pipeline: Arc<Pipeline<SData, Err>>) -> Self`**
    *   Defaults to `FanOutPolicy::CollectAll` and unbounded concurrency. Configure once, run many times (`run` takes `&self`).
*   **`pub fn policy(self, policy: FanOutPolicy) -> Self`**
*   **`pub fn custom_policy(self, is_satisfied: impl Fn(&FanOutResults<SData, Err>) -> bool + Send + Sync + 'static) -> Self`**
    *   Replaces any built-in policy. Evaluated once after every branch has settled, so unlike `FailFast` it cannot stop branches from starting.
*   **`pub fn max_concurrent(self, n: usize) -> Self`**. **Panics** if `n` is zero.
*   **`pub fn spawner(self, spawner: Arc<dyn TaskSpawner>) -> Self`**
    *   Runs each branch as a task on your executor instead of cooperatively on the caller's task. See "Spawning" below.
*   **`pub fn with_cancel(self, token: CancelToken) -> Self`**
    *   Stops starting new branches and drains in-flight ones, installing the token into every branch's context. See §10.
*   **`pub async fn run<I: IntoIterator<Item = SData>>(&self, items: I) -> FanOutResults<SData, Err>`**
    *   Wraps each item in its own `ContextData` and runs the pipeline over it. Each branch is a **full `Pipeline::run`**: its own `on_finish` ring, its own resource-bag release, its own run id. Always returns every outcome and never discards results, including when the policy is unsatisfied.

Concurrency is **cooperative by default**: orka depends on no async runtime, so branches are polled on the caller's task and progress while each other awaits. A branch that blocks the thread stalls its siblings. Supply a `spawner` for real parallelism.

### Spawning

*   **Trait `orka::TaskSpawner`**: `fn spawn(&self, task: SpawnedTask) -> SpawnHandle`, where both aliases are `Pin<Box<dyn Future<Output = ()> + Send>>`. The handle must resolve even when the task panicked or was aborted, so orka reports rather than hangs.
*   **`orka::TokioSpawner`** (feature `tokio`): the shipped `tokio::spawn` implementation. The feature pulls tokio in with only its `rt` feature; orka requires no runtime otherwise.
*   A branch spawns on its **first poll**, so `max_concurrent` and fail-fast still govern how many tasks exist at once and which items ever become tasks. `NotStarted` branches were never spawned.
*   Two semantics change with a spawner: a panicking branch is contained and reported as `OrkaError::FanOutBranchLost { index }` instead of unwinding the fan-out, and dropping the fan-out no longer cancels in-flight branches (they belong to the runtime and continue detached).

### Enum `orka::FanOutPolicy`

`FailFast` | `CollectAll` | `RequireAll` | `RequireAtLeast(usize)`. `Clone + Debug + PartialEq + Eq + Display`. Policies decide only whether the fan-out is *satisfied*; they never discard results.

`FailFast` is the only policy that acts before all branches settle, and it stops *starting* new branches while letting in-flight ones drain. It deliberately does not drop a running branch, which would skip its `on_finish` handlers and release its resources late. `FanOut::with_cancel` (section 10) is that same wind-down reached from outside rather than from a branch failure, and it composes with every policy.

### Results

*   **`pub enum FanOutItemOutcome<Err>`**: `Completed(PipelineResult)` (ran without error, whether its pipeline completed or stopped) | `Failed(Err)` (the branch's own **typed** error, not stringified) | `Cancelled` (started, then interrupted mid-run by `with_cancel`) | `NotStarted` (FailFast or a cancellation tripped first; the branch ran no code and its context holds the untouched input). Helpers `is_success`, `is_failure`, `is_cancelled`, `is_not_started`. Marked `#[non_exhaustive]`.
*   **`pub struct FanOutItem<SData, Err> { pub index: usize, pub context: ContextData<SData>, pub outcome: FanOutItemOutcome<Err> }`**
*   **`pub struct FanOutResults<SData, Err>`**, always in **input order** regardless of completion order:
    *   `items() -> &[FanOutItem<SData, Err>]`, `len()`, `is_empty()`
    *   `succeeded()`, `failed()`, `stopped()` (a subset of succeeded), `cancelled()`, `not_started()`
    *   `was_cancelled() -> bool`: whether the fan-out's token fired. Separate from `cancelled()` being non-zero, because a fan-out cancelled before any branch started has zero cancelled branches and every branch `NotStarted`.
    *   `oks() -> impl Iterator<Item = &ContextData<SData>>`, `cloned_oks() -> Vec<SData>` where `SData: Clone`
    *   `errors() -> impl Iterator<Item = (usize, &Err)>`: each failure with its **input index**, so callers can report which item failed rather than only how many
    *   `policy()`, `satisfied()`
    *   `into_first_error(self) -> Option<Err>` and `into_control(self) -> Result<PipelineControl, Err>` (consuming, since `Err` need not be `Clone`; read what you need first). `into_control` returns `Ok(Stop)` when `was_cancelled()`, `Ok(Continue)` when satisfied, otherwise the first branch's typed error, falling back to `OrkaError::FanOutPolicyUnmet` when the policy is unmet with no branch failure (`RequireAll` over branches that all stopped).
    *   A cancelled fan-out is **unmet under every built-in policy**, `CollectAll` included: "run everything and always report satisfied" is a claim about a fan-out that was allowed to run everything.
    *   `Display`: `"5 item(s): 3 succeeded, 1 failed, 0 cancelled, 1 not started (RequireAll: unmet)"`

## 10. Cancellation

Out-of-band run cancellation, the counterpart to in-band `PipelineControl::Stop`. Cooperative: stops new work starting, never drops an in-flight future.

### Struct `orka::core::cancel::CancelToken`

`Clone + Default + Debug`. Every `ContextData` carries one.

*   **`pub fn new() -> Self`**
*   **`pub fn cancel(&self)`**
    *   Sets the token and wakes every waiter. Idempotent.
*   **`pub fn is_cancelled(&self) -> bool`**
*   **`pub fn cancelled(&self) -> Cancelled`**

### Struct `orka::core::cancel::Cancelled`

`Future<Output = ()>`, `Send + 'static`. Returned by `CancelToken::cancelled`. Resolves once the token is cancelled, never otherwise.

### Reading the token

*   **`pub fn ContextData::cancellation(&self) -> CancelToken`**
    *   Always present, so no `Option`. Without a `run_with_cancel*` the token is unreachable: `is_cancelled()` stays false, `cancelled()` never resolves.
    *   `cancel()` on it from inside a handler cancels the current run.

### Running with a token

*   **`pub async fn Pipeline::run_with_cancel(&self, ctx_data: ContextData<TData>, token: CancelToken) -> Result<PipelineResult, Err>`**
*   **`pub async fn Pipeline::run_with_cancel_and_outcome(&self, ctx_data: ContextData<TData>, token: CancelToken) -> (Result<PipelineResult, Err>, RunOutcome)`**
*   **`pub async fn Pipeline::run_with_observer_and_cancel(&self, ctx_data: ContextData<TData>, observer: Arc<dyn PipelineObserver>, token: CancelToken) -> (Result<PipelineResult, Err>, RunOutcome)`**
*   **`pub async fn Orka::run_with_cancel<TData>(&self, ctx_data: ContextData<TData>, token: CancelToken) -> Result<PipelineResult, ApplicationError>`**
*   **`pub async fn Orka::run_with_cancel_and_outcome<TData>(&self, ctx_data: ContextData<TData>, token: CancelToken) -> (Result<PipelineResult, ApplicationError>, RunOutcome)`**
    *   The token is installed into `ctx_data`, so it survives the registry's type erasure. A `register_runner` registration honours it only if its `PipelineRunner` reaches the core run loop.
    *   Fan-out branches and conditional sub-pipelines inherit it.

### Semantics

*   Checked at each step boundary. A cancelled run stops before its next step, then takes the ordinary exit: `on_finish` fires in full, resource bag releases.
*   Reports `PipelineResult::Cancelled` / `RunOutcome::Cancelled`, not `Stopped`.
*   `PipelineControl::Stop` returned while the token is set reports as `Cancelled`.
*   Emits `TraceEventKind::RunCancelled { step, index }`, then `RunFinished { outcome: Cancelled }`.
*   `resolve_plan` on a cancelled context yields `PlannedAction::Cancelled` for every step.
*   Cancellation during the `on_finish` ring is ignored.
*   Bounds starting work, not finishing it. For a hard bound on one await see `orka::timed` (§8).

### Fan-out

*   **`pub fn FanOut::with_cancel(self, token: CancelToken) -> Self`**
    *   Stops starting new branches, drains in-flight ones, installs the token into every branch context. Composes with every policy.
*   `FanOutItemOutcome::Cancelled` (started, interrupted) versus `NotStarted` (ran no code); `FanOutResults::cancelled()`, `not_started()`, `was_cancelled()`. Unmet under every built-in policy; `into_control()` yields `Ok(PipelineControl::Stop)`. See §9.
