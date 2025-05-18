# II. Detailed Usage and API Overview

**1. Guide Introduction & Core Concepts Recap:**

*   **(a) Guide Purpose:** This guide provides a comprehensive overview of the Orka workflow engine, detailing its core concepts, API, and best practices for building robust, multi-step asynchronous processes in Rust.
*   **(b) Table of Contents (Suggest Main Sections):**
    1.  **Introduction to Orka**
        *   What is Orka?
        *   Why use a workflow engine?
    2.  **Core Concepts**
        *   Pipelines (`Pipeline<TData, Err>`)
        *   Context Data (`ContextData<T>`)
        *   Handlers (`Handler<TData, Err>`)
        *   Steps (`StepDef<TData>`) and Execution Phases (Before, On, After)
        *   Pipeline Control (`PipelineControl`, `PipelineResult`)
    3.  **Getting Started: A Simple Pipeline**
        *   Defining `TData` and `Err`
        *   Creating a `Pipeline`
        *   Registering Handlers (`on_root`)
        *   Running the Pipeline
    4.  **Advanced Pipeline Features**
        *   Sub-Context Extraction (`set_extractor`, `on<SData>`)
        *   Step Manipulation (Adding, Removing, Optional, Skip Conditions)
    5.  **Conditional Execution: Branching Workflows**
        *   Introduction to `ConditionalScopeBuilder`
        *   Defining Scopes: Static vs. Dynamic Pipelines
        *   Extractors and Conditions for Scopes
        *   Scoped Pipeline Context (`SData`) and Error Handling
        *   Finalizing Conditional Steps
    6.  **Managing Multiple Pipelines: The Orka Registry**
        *   The `Orka<ApplicationError>` Registry
        *   Registering and Running Pipelines by `TData` Type
    7.  **Error Handling in Depth**
        *   `OrkaError` (Framework Errors)
        *   Integrating with Application-Specific Errors (`From<OrkaError>`)
        *   Error Propagation in Main and Scoped Pipelines
    8.  **Best Practices & Patterns**
        *   Managing `ContextData` Locks
        *   Designing Idempotent Handlers (where applicable)
        *   Structuring Complex Workflows
    9.  **Examples** (Link to `examples/` directory)
    10. **API Reference** (Link to `docs.rs`)
    11. **Contributing**
*   **(c) Core Concepts Deep Dive:**
    *   **`Pipeline<TData, Err>`:** The fundamental building block. It encapsulates an ordered sequence of named `StepDef`s. `TData` is the shared state for the entire pipeline, wrapped in `ContextData<TData>`. `Err` is the application-defined error type for this pipeline's handlers, which must be `From<OrkaError>`. Each step can have handlers for `before`, `on` (main logic), and `after` phases.
    *   **`ContextData<T>`:** An `Arc<RwLock<T>>` providing shared ownership and interior mutability for pipeline state. It's crucial to manage lock acquisition (`read()`, `write()`) and ensure guards are dropped before any `.await` point to prevent deadlocks. Cloning `ContextData` clones the `Arc`, allowing efficient sharing.
    *   **`Handler<TData, Err>`:** A type alias for `Box<dyn Fn(ContextData<TData>) -> Pin<Box<dyn Future<Output = Result<PipelineControl, Err>> + Send>> + Send + Sync>`. Handlers are asynchronous, take the pipeline's context data, and return either `PipelineControl::Continue` to proceed or `PipelineControl::Stop` to halt execution, wrapped in a `Result` with the pipeline's `Err` type.
    *   **`StepDef<TData>`:** Defines a named step, its optionality, and an optional `SkipCondition<TData>` (a closure `Fn(ContextData<TData>) -> bool`) to dynamically skip the step.
    *   **`ConditionalScopeBuilder` & Scoped Pipelines:** This mechanism allows a single step in a main `Pipeline<TData, Err>` to conditionally execute one of several *scoped pipelines*. These scoped pipelines are themselves full `Pipeline<SData, Err>` instances (note: they now also use the main pipeline's `Err` type after our redesign). They operate on an `SData` (scoped context data) extracted from `TData`. This enables powerful dynamic workflow branching. The `PipelineProvider` trait defines how these scoped pipelines are obtained (statically or via a dynamic factory).
    *   **`Orka<ApplicationError>` Registry:** A container for multiple `Pipeline` definitions, keyed by their `TData`'s `TypeId`. It allows an application to define various workflows and run them by providing the corresponding initial context data. `ApplicationError` is the global error type for the registry, and must be `From<OrkaError>` and also `From<PipelineHandlerError>` for any pipeline registered.

**2. Quick Start Examples (2-3 examples):**

*   **Example 1: Basic Sequential Task Pipeline**
    *   **Use Case:** Demonstrates creating a simple pipeline with a few steps that execute in order, modifying a shared counter in `ContextData`.
    *   **Code Example:**
        ```rust
        use orka::{Pipeline, ContextData, PipelineControl, OrkaError, OrkaResult, Orka};
        use std::sync::Arc;
        use tracing::info; // For logging

        // Define Context Data
        #[derive(Clone, Debug, Default)]
        struct MySimpleContext {
            counter: i32,
            message: String,
        }

        // Define Application Error (must be From<OrkaError>)
        #[derive(Debug, thiserror::Error)]
        enum MyAppError {
            #[error("Orka framework error: {0}")]
            Orka(#[from] OrkaError),
            #[error("Task failed: {0}")]
            Task(String),
        }

        async fn run_simple_pipeline_example() -> Result<(), MyAppError> {
            // Initialize tracing (optional, for example output)
            // tracing_subscriber::fmt::init();

            // 1. Define the pipeline
            let mut pipeline = Pipeline::<MySimpleContext, MyAppError>::new(&[
                ("initialize", false, None),
                ("increment", false, None),
                ("finalize", false, None),
            ]);

            // 2. Register handlers
            pipeline.on_root("initialize", |ctx_data: ContextData<MySimpleContext>| {
                Box::pin(async move {
                    let mut guard = ctx_data.write();
                    guard.counter = 0;
                    guard.message = "Initialized".to_string();
                    info!("Step Initialize: Counter set to {}, Message: '{}'", guard.counter, guard.message);
                    Ok(PipelineControl::Continue)
                })
            });

            pipeline.on_root("increment", |ctx_data: ContextData<MySimpleContext>| {
                Box::pin(async move {
                    let mut guard = ctx_data.write();
                    guard.counter += 1;
                    // Example of a handler-specific error
                    if guard.counter > 5 { // Arbitrary condition for failure
                        return Err(MyAppError::Task("Counter exceeded limit in increment step".to_string()));
                    }
                    info!("Step Increment: Counter incremented to {}", guard.counter);
                    Ok(PipelineControl::Continue)
                })
            });

            pipeline.on_root("finalize", |ctx_data: ContextData<MySimpleContext>| {
                Box::pin(async move {
                    let guard = ctx_data.read();
                    guard.message.push_str(" and Finalized");
                    info!("Step Finalize: Message: '{}'", guard.message);
                    Ok(PipelineControl::Continue)
                })
            });

            // 3. Create initial context
            let initial_context = ContextData::new(MySimpleContext::default());

            // 4. Run the pipeline (can be run directly or via Orka registry)
            info!("Running pipeline directly...");
            let result = pipeline.run(initial_context.clone()).await?;
            info!("Pipeline direct run result: {:?}", result);

            let final_guard = initial_context.read();
            assert_eq!(final_guard.counter, 1);
            assert_eq!(final_guard.message, "Initialized and Finalized");
            info!("Final Context: Counter = {}, Message = '{}'", final_guard.counter, final_guard.message);

            Ok(())
        }

        // To run this (e.g., in a test or main):
        // #[tokio::main]
        // async fn main() { run_simple_pipeline_example().await.unwrap(); }
        ```

*   **Example 2: Conditional Logic with a Scoped Pipeline**
    *   **Use Case:** Demonstrates a main pipeline that, based on a condition in its context, either executes Scoped Pipeline A or Scoped Pipeline B.
    *   **Code Example:**
        ```rust
        use orka::{
            Pipeline, ContextData, PipelineControl, OrkaError, ConditionalScopeBuilder, Orka,
            PipelineResult as OrkaPipelineResult, // Renamed to avoid conflict
        };
        use std::sync::Arc;
        use tracing::info;

        // --- Shared Error and Contexts ---
        #[derive(Debug, thiserror::Error)]
        enum BranchingError {
            #[error("Orka framework error: {0}")]
            Orka(#[from] OrkaError),
            #[error("Scoped task failed: {0}")]
            ScopedTask(String),
            #[error("Main task failed: {0}")]
            MainTask(String),
        }

        #[derive(Clone, Debug, Default)]
        struct MainBranchingContext {
            branch_condition: bool,
            shared_value: i32,
            log: String,
        }

        #[derive(Clone, Debug, Default)]
        struct ScopedDataContextA {
            input_value: i32,
            processed_value_a: i32,
        }
        #[derive(Clone, Debug, Default)]
        struct ScopedDataContextB {
            input_value: i32,
            processed_value_b: i32,
        }

        // --- Scoped Pipeline Factories ---
        async fn factory_a(
            _main_ctx: ContextData<MainBranchingContext>,
        ) -> Result<Arc<Pipeline<ScopedDataContextA, BranchingError>>, OrkaError> {
            let mut p_a = Pipeline::new(&[("process_a", false, None)]);
            p_a.on_root("process_a", |s_ctx: ContextData<ScopedDataContextA>| Box::pin(async move {
                let mut s_guard = s_ctx.write();
                s_guard.processed_value_a = s_guard.input_value * 10;
                info!("Scoped Pipeline A: Processed {} -> {}", s_guard.input_value, s_guard.processed_value_a);
                Ok(PipelineControl::Continue)
            }));
            Ok(Arc::new(p_a))
        }
        async fn factory_b(
            _main_ctx: ContextData<MainBranchingContext>,
        ) -> Result<Arc<Pipeline<ScopedDataContextB, BranchingError>>, OrkaError> {
            let mut p_b = Pipeline::new(&[("process_b", false, None)]);
            p_b.on_root("process_b", |s_ctx: ContextData<ScopedDataContextB>| Box::pin(async move {
                let mut s_guard = s_ctx.write();
                s_guard.processed_value_b = s_guard.input_value + 100;
                info!("Scoped Pipeline B: Processed {} -> {}", s_guard.input_value, s_guard.processed_value_b);
                Ok(PipelineControl::Continue)
            }));
            Ok(Arc::new(p_b))
        }

        async fn run_conditional_pipeline_example(use_branch_a: bool) -> Result<MainBranchingContext, BranchingError> {
            // tracing_subscriber::fmt::init();
            let mut pipeline = Pipeline::<MainBranchingContext, BranchingError>::new(&[
                ("setup", false, None),
                ("conditional_processing", false, None),
                ("integrate_results", false, None),
            ]);

            pipeline.on_root("setup", |ctx| Box::pin(async move {
                ctx.write().shared_value = 5;
                Ok(PipelineControl::Continue)
            }));

            pipeline.conditional_scopes_for_step("conditional_processing")
                .add_dynamic_scope( // Use Scoped Pipeline A
                    factory_a,
                    |main_ctx| { // Extractor for ScopedDataContextA
                        let val = main_ctx.read().shared_value;
                        Ok(ContextData::new(ScopedDataContextA { input_value: val, ..Default::default() }))
                    }
                )
                .on_condition(|main_ctx| main_ctx.read().branch_condition) // Condition for A
                .add_dynamic_scope( // Use Scoped Pipeline B
                    factory_b,
                    |main_ctx| { // Extractor for ScopedDataContextB
                        let val = main_ctx.read().shared_value;
                        Ok(ContextData::new(ScopedDataContextB { input_value: val, ..Default::default() }))
                    }
                )
                .on_condition(|main_ctx| !main_ctx.read().branch_condition) // Condition for B
                .finalize_conditional_step(false);

            pipeline.on_root("integrate_results", |ctx| Box::pin(async move {
                // This step would typically look at what the scoped pipeline put into SData
                // and integrate it back into TData if necessary.
                // For this example, the scoped pipelines just log.
                // We'll add a log message to the main context.
                let mut guard = ctx.write();
                if guard.branch_condition {
                    guard.log.push_str("Branch A was chosen. ");
                } else {
                    guard.log.push_str("Branch B was chosen. ");
                }
                info!("Main Pipeline: Integrating results. Log: {}", guard.log);
                Ok(PipelineControl::Continue)
            }));

            let initial_context = ContextData::new(MainBranchingContext {
                branch_condition: use_branch_a,
                ..Default::default()
            });
            pipeline.run(initial_context.clone()).await?;
            
            let final_data = initial_context.read().clone(); // Clone the inner data
            Ok(final_data)
        }
        
        // To run this (e.g., in a test or main):
        // #[tokio::main]
        // async fn main() {
        //     let result_a = run_conditional_pipeline_example(true).await.unwrap();
        //     info!("Final result (Branch A): {:?}", result_a);
        //     assert!(result_a.log.contains("Branch A"));
        //
        //     let result_b = run_conditional_pipeline_example(false).await.unwrap();
        //     info!("Final result (Branch B): {:?}", result_b);
        //     assert!(result_b.log.contains("Branch B"));
        // }
        ```

**3. Configuration System:**

*   **(a) Overview:** Orka itself does not have a complex configuration system exposed to the end-user for its core operations. Configuration primarily happens through:
    *   **Pipeline Definition:** Constructing `Pipeline` instances with specific steps.
    *   **Handler Registration:** Providing closures or functions as handlers.
    *   **`ConditionalScopeBuilder` API:** Fluent API calls to define conditional logic.
    *   **`Orka` Registry:** Registering pipelines.
    Application-specific configuration (like database URLs, API keys for services called by handlers) is managed by the application using Orka, not by Orka itself.
*   **(b) Primary Configuration Types:** Not applicable in the sense of library-wide config structs. Configuration is programmatic via the API.
*   **(c) Key Configuration Enums/Options:**
    *   `orka::core::control::PipelineControl`: (Enum) Used in `ConditionalScopeBuilder::if_no_scope_matches` to determine behavior. Variants: `Continue`, `Stop`.

**4. Main API Sections / Functional Areas:**

*   **A. Pipeline Definition & Execution**
    *   **Primary Types:** `orka::Pipeline<TData, Err>`, `orka::core::step::StepDef<TData>`, `orka::core::context_data::ContextData<T>`
    *   **Common Methods/Functions:**
        1.  **`Pipeline::new(step_defs: &[(&str, bool, Option<SkipCondition<TData>>)]) -> Self`**
            *   Constructs a new pipeline definition with an ordered list of named steps, their optionality, and skip conditions.
        2.  **`Pipeline::on_root<F, UserProvidedErr>(&mut self, step_name: &str, handler_fn: impl Fn(ContextData<TData>) -> F + Send + Sync + 'static)`** (and `before_root`, `after_root`)
            *   Where `F: Future<Output = Result<PipelineControl, UserProvidedErr>> + Send + 'static`, `UserProvidedErr: Into<Err> + Send + Sync + 'static`.
            *   Registers an asynchronous handler for a specific phase of a named step.
        3.  **`Pipeline::run(&self, ctx_data: ContextData<TData>) -> Future<Output = Result<PipelineResult, Err>>`**
            *   Asynchronously executes all defined steps and handlers of the pipeline using the provided initial context.
    *   **Supporting Types:**
        *   `orka::core::context::Handler<TData, Err>` (Type Alias): The signature for handler functions.
        *   `orka::core::control::PipelineControl` (Enum): Output of handlers to control flow.
        *   `orka::core::control::PipelineResult` (Enum): Final outcome of `Pipeline::run`.

*   **B. Conditional Workflow Branching**
    *   **Primary Types:** `orka::conditional::builder::ConditionalScopeBuilder<'pipeline, TData, Err>`, `orka::conditional::builder::ConditionalScopeConfigurator<...>`, `orka::conditional::provider::PipelineProvider<TData, SData, MainErr>` (Trait)
    *   **Common Methods/Functions:**
        1.  **`Pipeline::conditional_scopes_for_step(&mut self, step_name: &str) -> ConditionalScopeBuilder<TData, Err>`**
            *   Initiates the definition of conditional logic for a specified step in the main pipeline.
        2.  **`ConditionalScopeBuilder::add_dynamic_scope<SData, F, Fut>(self, pipeline_factory: F, extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static) -> ConditionalScopeConfigurator<...>`**
            *   Where `F: Fn(ContextData<TData>) -> Fut`, `Fut: Future<Output = Result<Arc<Pipeline<SData, Err>>, OrkaError>>`.
            *   Adds a conditional scope whose sub-pipeline (`Pipeline<SData, Err>`) is sourced from an asynchronous factory function; requires an extractor for the sub-context.
        3.  **`ConditionalScopeConfigurator::on_condition(mut self, condition_fn: impl Fn(ContextData<TData>) -> bool + Send + Sync + 'static) -> ConditionalScopeBuilder<TData, Err>`**
            *   Sets the boolean predicate that determines if the configured scope should be executed.
        4.  **`ConditionalScopeBuilder::finalize_conditional_step(self, optional_for_main_step: bool)`**
            *   Completes the conditional setup for the step, embedding the logic into the main pipeline.
    *   **Supporting Types:**
        *   `orka::conditional::provider::StaticPipelineProvider<SData, Err>`: Provides a pre-built scoped pipeline.
        *   `orka::conditional::provider::FunctionalPipelineProvider<TData, SData, Err, F, Fut>`: Provides a scoped pipeline via a factory function.

*   **C. Pipeline Registry & Management**
    *   **Primary Types:** `orka::registry::Orka<ApplicationError>`
    *   **Common Methods/Functions:**
        1.  **`Orka::new() -> Self`**
            *   Creates a new, empty Orka pipeline registry.
        2.  **`Orka::register_pipeline<TData, PipelineHandlerError>(&self, pipeline: Pipeline<TData, PipelineHandlerError>)`**
            *   Where `PipelineHandlerError: From<OrkaError>`, `ApplicationError: From<PipelineHandlerError>`.
            *   Registers a fully defined pipeline with the registry, keyed by its `TData` type.
        3.  **`Orka::run<TData>(&self, ctx_data: ContextData<TData>) -> Future<Output = Result<PipelineResult, ApplicationError>>`**
            *   Looks up and executes the pipeline registered for the given `TData` type using the provided context.
    *   **Supporting Types:** None directly, but interacts with `Pipeline` and `ContextData`.

**5. Specialized Features:**

*   **Conditional Execution of Scoped Pipelines (Plugin-like Architecture)**
    *   **Concept & Purpose:** Allows a single step in a main pipeline to act as a dispatcher, dynamically choosing one of several specialized sub-pipelines to execute based on runtime conditions. This is useful for scenarios like selecting different payment gateways, handling various event types from a webhook, or implementing strategy patterns.
    *   **Key Types/Methods:**
        *   `Pipeline::conditional_scopes_for_step(...) -> ConditionalScopeBuilder<...>`: Entry point.
        *   `ConditionalScopeBuilder::add_dynamic_scope<SData, F, Fut>(...)`: Adds a scope where the sub-pipeline (`Pipeline<SData, Err>`) is created by `pipeline_factory: F` (where `Fut::Output = Result<Arc<Pipeline<SData, Err>>, OrkaError>`). The `extractor_fn: impl Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError>` provides the sub-pipeline's context.
        *   `ConditionalScopeConfigurator::on_condition(...)`: Sets the condition for a scope.
        *   `PipelineProvider<TData, SData, MainErr>` (Trait): Abstract way to get a scoped pipeline.

**6. Error Handling:**

*   **(a) Primary Error Type(s):**
    *   `orka::error::OrkaError` (Enum): The main error type for the Orka framework itself. It covers errors related to pipeline configuration, execution integrity (e.g., missing handlers, type mismatches), and failures within framework components like extractors or pipeline providers.
*   **(b) Key Error Variants (of `OrkaError`):**
    *   `StepNotFound { step_name: String }`: A referenced pipeline step was not defined.
    *   `HandlerMissing { step_name: String }`: A non-optional step lacks necessary handlers.
    *   `ExtractorFailure { step_name: String, source: anyhow::Error }`: A sub-context extractor function failed.
    *   `PipelineProviderFailure { step_name: String, source: anyhow::Error }`: A `PipelineProvider` failed to yield a scoped pipeline.
    *   `TypeMismatch { step_name: String, expected_type: String }`: An error during context downcasting.
    *   `HandlerError { source: anyhow::Error }`: A wrapper for errors originating from user code that are converted into `OrkaError` (e.g., errors from services called by handlers within certain Orka-internal pipeline types).
    *   `ConfigurationError { step_name: String, message: String }`: Generic configuration issues (e.g., pipeline not found in registry).
    *   `Internal(String)`: For miscellaneous internal Orka errors.
*   **(c) Standard Result Alias:**
    *   `pub type OrkaResult<T, E = OrkaError> = std::result::Result<T, E>;`
    *   User applications are expected to define their own error types (e.g., `AppError`) and implement `From<OrkaError>` for them, allowing Orka framework errors to be integrated into the application's error handling scheme. Pipelines (`Pipeline<TData, Err>`) and the Orka registry (`Orka<ApplicationError>`) are generic over these application-defined error types.