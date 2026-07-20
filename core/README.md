# Orka Workflow Engine

[![Crates.io](https://img.shields.io/crates/v/orka.svg)](https://crates.io/crates/orka)
[![Docs.rs](https://docs.rs/orka/badge.svg)](https://docs.rs/orka)

Orka is an asynchronous, pluggable, and type-safe workflow engine for Rust, designed to orchestrate complex multi-step business processes with robust context management and conditional logic. It simplifies the development of intricate, stateful workflows by providing a clear structure for defining steps, managing shared data, handling errors consistently, and enabling dynamic execution paths.

Upgrading from 0.1? See **[MIGRATION.md](MIGRATION.md)**.

## Key Features

*   **Type-Safe Pipelines:** Define workflows (`Pipeline<TData, Err>`) generic over shared context data (`TData`) and a specific error type (`Err`), ensuring compile-time safety throughout your process.
*   **Asynchronous Handlers:** Execute pipeline steps with `async` handlers, suited to non-blocking I/O.
*   **Shared Context Management:** `ContextData<T>` (`Arc<RwLock<T>>`) gives safe, shared, mutable access to pipeline state across handlers.
*   **Conditional Logic & Scoped Pipelines:** `ConditionalScopeBuilder` defines dynamic branching, executing sub-pipelines (`Pipeline<SData, Err>`) chosen at runtime, sourced statically or from an async factory.
*   **Flexible Error Handling:** Pipelines are generic over their error type; `OrkaError` converts into it via `From<OrkaError>`.
*   **Sub-Context Extraction:** Handlers can operate on a type-safe sub-section (`SData`) of the main context, optionally merging their work back.
*   **Setup Validation:** `Pipeline::validate()` reports configuration mistakes before the first run.
*   **Pipeline Registry:** Manage and run multiple pipeline definitions through the `Orka<ApplicationError>` type-keyed registry.

## Getting Started

### Prerequisites

*   **Rust:** A recent stable toolchain. See [rustup.rs](https://rustup.rs/).
*   **Tokio:** Orka runs on the Tokio async runtime.

### Installation

```toml
[dependencies]
orka = "0.2"
tokio = { version = "1", features = ["full"] }
thiserror = "2"
```

### Quick Start

```rust
use orka::prelude::*;

#[derive(Clone, Debug, Default)]
struct OrderContext {
  order_id: String,
  total: u64,
  paid: bool,
}

#[derive(Debug, thiserror::Error)]
enum AppError {
  #[error(transparent)]
  Orka(#[from] OrkaError),
  #[error("payment declined: {0}")]
  Declined(String),
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
  let mut pipeline = Pipeline::<OrderContext, AppError>::new(["price", "charge", "notify"]);

  pipeline
    .optional("notify")
    .skip_if("charge", |ctx| ctx.read().total == 0)
    .on_root("price", |ctx| async move {
      ctx.write().total = 4_200;
      Ok(PipelineControl::Continue)
    })
    .on_root("charge", |ctx| async move {
      let total = ctx.read().total;
      if total > 10_000 {
        return Err(AppError::Declined("over limit".into()));
      }
      ctx.write().paid = true;
      Ok(PipelineControl::Continue)
    })
    .on_root("notify", |ctx| async move {
      println!("order {} paid", ctx.read().order_id);
      Ok(PipelineControl::Continue)
    });

  let orka = Orka::<AppError>::new();
  orka.register_pipeline(pipeline)?;

  let ctx = ContextData::new(OrderContext::default());
  match orka.run(ctx.clone()).await? {
    PipelineResult::Completed => println!("done, paid = {}", ctx.read().paid),
    PipelineResult::Stopped => println!("stopped early"),
  }

  Ok(())
}
```

Step names come first; optionality and skip conditions are chained afterwards. Every registration method returns `&mut Self`, so setup reads as one chain. Handlers are plain `async move` blocks returning `Result<PipelineControl, Err>` — no `Box::pin`, no turbofish.

## Documentation

*   **[Usage Guide (README.USAGE.md)](README.USAGE.md):** Walkthrough of core concepts, sub-contexts, conditional branching, and error handling.
*   **[API Reference (API_REFERENCE.md)](API_REFERENCE.md):** Signature-level reference.
*   **[Migration Guide (MIGRATION.md)](MIGRATION.md):** Upgrading from 0.1 to 0.2.
*   **[docs.rs/orka](https://docs.rs/orka):** Generated API documentation.
*   **[Examples (`examples/`)](examples):** Runnable examples, from `basic_pipeline` through `conditional_dynamic`.

## Contributing

Contributions are welcome — bug reports, feature suggestions, documentation improvements, or code. Open an issue or pull request on [GitHub](https://github.com/excsn/orka).

## License

Orka is distributed under the terms of the **Mozilla Public License, v. 2.0**.

A copy of the license is available in the [LICENSE](LICENSE) file, or at http://mozilla.org/MPL/2.0/.
