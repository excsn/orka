# Orka Workflow Engine

[![Crates.io](https://img.shields.io/crates/v/orka.svg)](https://crates.io/crates/orka)
[![Docs.rs](https://docs.rs/orka/badge.svg)](https://docs.rs/orka)

Orka is an asynchronous, pluggable, and type-safe workflow engine for Rust, designed to orchestrate complex multi-step business processes with robust context management and conditional logic. It simplifies the development of intricate, stateful workflows by providing a clear structure for defining steps, managing shared data, handling errors consistently, and enabling dynamic execution paths, thereby improving code organization and maintainability for complex operations.

## Key Features

*   **🚀 Type-Safe Pipelines:** Define workflows (`Pipeline<TData, Err>`) generic over shared context data (`TData`) and a specific error type (`Err`), ensuring compile-time safety throughout your process.
*   **⚡ Asynchronous Handlers:** Execute pipeline steps with `async fn` handlers, perfect for non-blocking I/O and efficient resource use.
*   **📦 Shared Context Management:** Utilize `ContextData<T>` (`Arc<RwLock<T>>`) for safe, shared, and mutable access to pipeline state across handlers, with enforced lock guard discipline.
*   **🌿 Conditional Logic & Scoped Pipelines:** Employ a powerful `ConditionalScopeBuilder` to define dynamic branching, executing isolated sub-pipelines (`Pipeline<SData, Err>`) based on runtime conditions. Supports dynamic or static sourcing of these scoped pipelines.
*   **🛡️ Flexible Error Handling:** Integrate Orka with your application's error ecosystem. Pipelines are generic over their error type, and the core `OrkaError` can be seamlessly converted (via `From<OrkaError>`).
*   **🔍 Sub-Context Extraction:** Allow handlers to operate on specific, type-safe sub-sections (`SData`) of the main pipeline's context (`TData`) through extractors.
*   **🏛️ Pipeline Registry:** Manage and run multiple distinct pipeline definitions within your application using the `Orka<ApplicationError>` type-keyed registry.

## Getting Started

### Prerequisites

*   **Rust:** A recent stable Rust toolchain. See [rustup.rs](https://rustup.rs/).
*   **Tokio:** Orka leverages Tokio for its asynchronous runtime. Ensure your project uses Tokio.

### Installation

Add Orka to your `Cargo.toml` dependencies:

```toml
[dependencies]
orka = "0.1.0" # Replace with the latest version from crates.io
tokio = { version = "1", features = ["full"] } # Orka requires a Tokio runtime
# Add other necessary crates like tracing, serde, thiserror, etc.
```

### Quick Overview

1.  **Define Context Data:** Create a struct for your pipeline's shared state (e.g., `MyWorkflowData`).
2.  **Define Error Type:** Create an application-specific error enum that implements `From<orka::OrkaError>`.
3.  **Create a Pipeline:** Instantiate `orka::Pipeline<MyWorkflowData, MyAppError>::new(...)` with named steps.
4.  **Register Handlers:** Use methods like `pipeline.on_root(...)` to attach asynchronous logic to steps.
    ```rust
    use orka::{Pipeline, ContextData, PipelineControl, OrkaError};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct MyContext { count: i32 }
    #[derive(Debug, thiserror::Error)]
    enum MyError { #[error(transparent)] Orka(#[from] OrkaError), /* ... */ }

    let mut pipeline = Pipeline::<MyContext, MyError>::new(&[("step1", false, None)]);
    pipeline.on_root("step1", |ctx: ContextData<MyContext>| Box::pin(async move {
        ctx.write().count += 1;
        Ok(PipelineControl::Continue)
    }));
    ```
5.  **(Optional) Define Conditional Logic:** Use `pipeline.conditional_scopes_for_step(...)` for branching.
6.  **(Optional) Use the Registry:** Create an `orka::Orka<MyAppError>` instance and register your pipeline(s).
7.  **Run the Pipeline:**
    ```rust
    # async {
    # use orka::{Pipeline, ContextData, PipelineControl, OrkaError, Orka, PipelineResult};
    # #[derive(Clone, Default)] struct MyContext { count: i32 }
    # #[derive(Debug, thiserror::Error)] enum MyError { #[error(transparent)] Orka(#[from] OrkaError),}
    # let mut pipeline = Pipeline::<MyContext, MyError>::new(&[("step1", false, None)]);
    # pipeline.on_root("step1", |ctx: ContextData<MyContext>| Box::pin(async move { Ok(PipelineControl::Continue) }));
    let initial_data = ContextData::new(MyContext::default());
    let outcome = pipeline.run(initial_data.clone()).await;
    // Or, if using the registry:
    // let orka_registry = Orka::<MyError>::new();
    // orka_registry.register_pipeline(pipeline);
    // let outcome = orka_registry.run(initial_data.clone()).await;

    match outcome {
        Ok(PipelineResult::Completed) => println!("Pipeline completed! Count: {}", initial_data.read().count),
        Ok(PipelineResult::Stopped) => println!("Pipeline stopped."),
        Err(e) => println!("Pipeline failed: {:?}", e),
    }
    # };
    ```

## Documentation

*   **[Orka Usage Guide (README.GUIDE.md)](README.GUIDE.md):** For a detailed walkthrough of core concepts, advanced features, and best practices.
*   **[API Reference (docs.rs/orka)](https://docs.rs/orka):** Full, detailed API documentation.
*   **[Examples (`examples/`)](../examples):** Check out the `ecommerce_app` for a practical application of Orka.

## Contributing

Contributions are highly welcome! Whether it's bug reports, feature suggestions, documentation improvements, or code contributions, please feel free to open an issue or pull request on [GitHub](https://github.com/excsn/orka).

## License

Orka is distributed under the terms of the **Mozilla Public License, v. 2.0**.

A copy of the license is available in the [LICENSE](LICENSE) file, or at http://mozilla.org/MPL/2.0/.