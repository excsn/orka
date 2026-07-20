# Orka Usage Guide

This guide walks through building workflows with Orka: defining pipelines and handlers, working on sub-contexts, branching with conditional scopes, and integrating Orka's errors with your own.

For signature-level detail see [API_REFERENCE.md](API_REFERENCE.md). For runnable code see [`examples/`](examples).

## Table of Contents

1. [Core Concepts](#1-core-concepts)
2. [A First Pipeline](#2-a-first-pipeline)
3. [Steps: Optionality, Skipping, and Mutation](#3-steps-optionality-skipping-and-mutation)
4. [Handlers and Control Flow](#4-handlers-and-control-flow)
5. [Sub-Contexts: Extractors and Merging](#5-sub-contexts-extractors-and-merging)
6. [Conditional Scopes: Branching Workflows](#6-conditional-scopes-branching-workflows)
7. [The Orka Registry](#7-the-orka-registry)
8. [Validation and Setup Errors](#8-validation-and-setup-errors)
9. [Error Handling](#9-error-handling)
10. [Best Practices](#10-best-practices)

## 1. Core Concepts

**`Pipeline<TData, Err>`** is an ordered sequence of named steps. `TData` is the shared state the whole pipeline operates on; `Err` is the error type its handlers return, which must implement `From<OrkaError>` so framework failures can flow through it.

**`ContextData<T>`** is `Arc<RwLock<T>>`. Cloning it shares the same underlying data. Lock guards from `read()` and `write()` are blocking and **must be dropped before any `.await`**.

**Steps** each have three phases — `before`, `on`, `after` — and any number of handlers may be registered per phase. Handlers within a phase run in registration order.

**`PipelineControl`** is what a handler returns to steer execution: `Continue` or `Stop`. **`PipelineResult`** is the outcome of a whole run: `Completed` or `Stopped`.

**Conditional scopes** let one step dispatch to one of several sub-pipelines (`Pipeline<SData, Err>`) chosen by runtime predicates.

**`Orka<ApplicationError>`** is a registry keyed by `TData`'s `TypeId`, so an application can hold many workflows and run them by handing over the matching context.

Import the common surface with:

```rust
use orka::prelude::*;
```

This brings in `Pipeline`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, and `Orka`. Advanced items — `Handler`, `ContextDataExtractorImpl`, the pipeline providers, and the conditional-scope builder types — are imported from the crate root when needed.

## 2. A First Pipeline

```rust
use orka::prelude::*;

#[derive(Clone, Debug, Default)]
struct ReportContext {
  rows: Vec<String>,
  summary: String,
}

#[derive(Debug, thiserror::Error)]
enum ReportError {
  #[error(transparent)]
  Orka(#[from] OrkaError),
  #[error("no rows to report")]
  Empty,
}

#[tokio::main]
async fn main() -> Result<(), ReportError> {
  let mut pipeline = Pipeline::<ReportContext, ReportError>::new(["load", "summarize", "publish"]);

  pipeline
    .on_root("load", |ctx| async move {
      ctx.write().rows = vec!["a".into(), "b".into()];
      Ok(PipelineControl::Continue)
    })
    .on_root("summarize", |ctx| async move {
      let count = ctx.read().rows.len();
      if count == 0 {
        return Err(ReportError::Empty);
      }
      ctx.write().summary = format!("{count} rows");
      Ok(PipelineControl::Continue)
    })
    .on_root("publish", |ctx| async move {
      println!("{}", ctx.read().summary);
      Ok(PipelineControl::Continue)
    });

  let ctx = ContextData::new(ReportContext::default());
  match pipeline.run(ctx.clone()).await? {
    PipelineResult::Completed => println!("completed"),
    PipelineResult::Stopped => println!("stopped early"),
  }

  Ok(())
}
```

`Pipeline::new` takes step names in execution order and accepts anything iterable of string-likes — `&["a", "b"]`, `["a", "b"]`, or a `Vec<String>` built at runtime:

```rust
let names: Vec<String> = config.stages.iter().map(|s| s.name.clone()).collect();
let pipeline = Pipeline::<ReportContext, ReportError>::new(names);
```

Every step starts out **required** with no skip condition.

## 3. Steps: Optionality, Skipping, and Mutation

Optionality and skip conditions are chained after construction:

```rust
pipeline
  .optional("notify")                                  // may have no handlers; scope errors swallowed
  .required("audit")                                   // back to the default
  .skip_if("validate", |ctx| ctx.read().already_valid) // evaluated at run time
  .clear_skip_condition("validate");
```

A **required** step with no handlers fails the run with `OrkaError::HandlerMissing`. An **optional** step with no handlers is simply skipped, and errors from its conditional scopes are swallowed so the pipeline continues.

A **skip condition** is checked immediately before the step runs. If it returns `true`, none of the step's `before`/`on`/`after` handlers execute.

Steps can be added and removed while building:

```rust
pipeline
  .insert_before_step("charge", "fraud_check")
  .insert_after_step("charge", "receipt")
  .optional("receipt")
  .remove_step("legacy_step");
```

Inserted steps are required by default; chain `.optional(..)` or `.skip_if(..)` to configure them. `remove_step` also drops every handler, extractor, and conditional configuration registered against that step, and is a no-op for an unknown name.

## 4. Handlers and Control Flow

Handlers are registered with `before_root`, `on_root`, and `after_root`. Each takes a step name and a closure receiving `ContextData<TData>` and returning a future:

```rust
pipeline
  .before_root("charge", |ctx| async move {
    tracing::info!(order = %ctx.read().order_id, "charging");
    Ok(PipelineControl::Continue)
  })
  .on_root("charge", |ctx| async move {
    let amount = ctx.read().total;              // guard dropped at end of statement
    let receipt = gateway::charge(amount).await?; // `?` converts via From
    ctx.write().receipt_id = receipt.id;
    Ok(PipelineControl::Continue)
  })
  .after_root("charge", |ctx| async move {
    ctx.write().log.push("charged".into());
    Ok(PipelineControl::Continue)
  });
```

The future must resolve to `Result<PipelineControl, Err>` where `Err` is the pipeline's own error type. Because `Err` is fixed by the pipeline, a bare `Ok(PipelineControl::Continue)` infers correctly, and `?` converts other error types through `From` as usual.

Returning `PipelineControl::Stop` halts immediately: no further handlers in the current step, and no further steps. The run resolves to `Ok(PipelineResult::Stopped)`.

Returning `Err(..)` aborts the run and propagates the error out of `run`.

### Locks and `.await`

`ContextData` guards are blocking. Never hold one across a suspension point:

```rust
// Wrong — guard is live across the await.
.on_root("fetch", |ctx| async move {
  let mut data = ctx.write();
  data.body = http::get(&data.url).await?;
  Ok(PipelineControl::Continue)
})

// Right — read what you need, drop the guard, then await.
.on_root("fetch", |ctx| async move {
  let url = ctx.read().url.clone();
  let body = http::get(&url).await?;
  ctx.write().body = body;
  Ok(PipelineControl::Continue)
})
```

## 5. Sub-Contexts: Extractors and Merging

A step can operate on a focused slice of the context instead of the whole thing. Register an extractor for the step, then an `on` handler typed to the extracted data.

`ContextData::project` is the idiomatic way to write an extractor: it takes a read lock, clones out the part you want, releases the lock, and hands back a new `ContextData`.

```rust
pipeline
  .set_extractor("validate_customer", |main: ContextData<OrderContext>| {
    Ok(main.project(|d| d.customer.clone()))
  })
  .on("validate_customer", |sub: ContextData<CustomerInfo>| async move {
    if !sub.read().email.contains('@') {
      return Err(OrderError::InvalidEmail);
    }
    sub.write().is_validated = true;
    Ok(PipelineControl::Continue)
  });
```

Annotating the closure parameter (`|sub: ContextData<CustomerInfo>|`) is what tells Orka which sub-context type you mean.

### Detached versus merging extractors

This is the most common source of surprise. **`set_extractor` produces a detached sub-context.** The sub-handler above sets `is_validated`, but it does so on its own `ContextData`, and the root `OrderContext` never sees it.

To fold the work back into the parent, use `set_extractor_with_merge` and supply a merge function `Fn(&mut TData, &SData)`:

```rust
pipeline
  .set_extractor_with_merge(
    "validate_customer",
    |main: ContextData<OrderContext>| Ok(main.project(|d| d.customer.clone())),
    |root, sub| root.customer = sub.clone(),
  )
  .on("validate_customer", |sub: ContextData<CustomerInfo>| async move {
    sub.write().is_validated = true;
    Ok(PipelineControl::Continue)
  });
```

The merge runs with a write lock on the root and a read lock on the sub-context, and **only when the sub-handler returns `Ok`** — a failed sub-handler leaves the root untouched.

Use the detached form when the sub-pipeline only reads, or when you deliberately want its mutations discarded. Use the merging form whenever its results matter. `examples/sub_context.rs` runs both side by side.

## 6. Conditional Scopes: Branching Workflows

A conditional step dispatches to one of several sub-pipelines. Scopes are tested in registration order, and the first whose condition holds is executed.

### Static scopes

Use `add_static_scope` when the sub-pipelines are built once up front:

```rust
use std::sync::Arc;

let card_pipeline: Arc<Pipeline<PaymentInfo, OrderError>> = Arc::new(build_card_pipeline());
let wire_pipeline: Arc<Pipeline<PaymentInfo, OrderError>> = Arc::new(build_wire_pipeline());

pipeline
  .conditional_scopes_for_step("pay")
  .add_static_scope(card_pipeline, |main: ContextData<OrderContext>| {
    Ok(main.project(|d| d.payment.clone()))
  })
  .with_merge(|root, sub| root.payment = sub.clone())
  .on_condition(|main| main.read().method == Method::Card)
  .add_static_scope(wire_pipeline, |main: ContextData<OrderContext>| {
    Ok(main.project(|d| d.payment.clone()))
  })
  .with_merge(|root, sub| root.payment = sub.clone())
  .on_condition(|main| main.read().method == Method::Wire)
  .if_no_scope_matches(PipelineControl::Continue)
  .finalize_conditional_step(false);
```

The chain reads: add a scope, optionally attach a merge, give it a condition, repeat. `.with_merge(..)` goes between `add_static_scope`/`add_dynamic_scope` and `on_condition`. Without it, the scope is detached exactly like a plain `set_extractor` — the sub-pipeline works on its own context and the main context sees nothing of it. With it, the sub-context is folded back after the scoped pipeline succeeds.

### Dynamic scopes

Use `add_dynamic_scope` when the sub-pipeline must be built per run — from a lookup, a tenant config, or anything else async:

```rust
async fn tenant_pipeline(
  main: ContextData<OrderContext>,
) -> Result<Arc<Pipeline<PaymentInfo, OrderError>>, OrkaError> {
  let tenant = main.read().tenant_id.clone();
  registry::lookup(&tenant)
    .await
    .map_err(|e| OrkaError::Internal(format!("no pipeline for tenant {tenant}: {e}")))
}

pipeline
  .conditional_scopes_for_step("pay")
  .add_dynamic_scope(tenant_pipeline, |main: ContextData<OrderContext>| {
    Ok(main.project(|d| d.payment.clone()))
  })
  .with_merge(|root, sub| root.payment = sub.clone())
  .on_condition(|main| main.read().is_multi_tenant)
  .finalize_conditional_step(false);
```

### Finalizing

`finalize_conditional_step(optional_for_main_step)` **must** terminate the chain — the collected scopes are discarded otherwise. Its argument sets the step's optionality: pass `true` and errors from a matched scope are swallowed and the pipeline continues; pass `false` and they propagate. `Pipeline::validate` reports a builder you forgot to finalize.

The step is created automatically if it does not already exist. Conditional scopes **append** their handler, so `on_root` handlers already registered on the same step still run.

`if_no_scope_matches(..)` sets what happens when no condition holds — `PipelineControl::Continue` by default.

## 7. The Orka Registry

`Orka<ApplicationError>` holds many pipelines, keyed by their context type:

```rust
let orka = Orka::<AppError>::new();

orka.register_pipeline(order_pipeline)?;   // keyed by OrderContext
orka.register_pipeline(refund_pipeline)?;  // keyed by RefundContext

let ctx = ContextData::new(OrderContext::default());
let outcome = orka.run(ctx.clone()).await?;
```

`register_pipeline` validates the pipeline and returns `OrkaResult<()>`, so setup mistakes surface at registration rather than on the first run. Pipelines are keyed by `TData`, so registering a second pipeline for the same context type replaces the first.

`ApplicationError` must be `From<OrkaError>` and `From<PipelineHandlerError>` for every pipeline registered — the registry converts each pipeline's error into the application error. When your handlers use `OrkaError` directly, `Orka::new_default()` is a convenience constructor for `Orka<OrkaError>`.

`Orka::run` looks up the pipeline by the type of the context you hand it. If nothing is registered for that type it returns `OrkaError::ConfigurationError`.

## 8. Validation and Setup Errors

Orka splits setup mistakes into two categories.

**Panics** — programming errors caught immediately at the call that made them:

*   Referring to a step name that does not exist (`on_root("typo", ..)`).
*   Declaring the same step name twice in `Pipeline::new`, or inserting a step that already exists.
*   Registering `on::<SData>` for a step with no extractor.

**`validate()`** — problems that are only visible once the whole pipeline is assembled:

```rust
pipeline.validate()?;
```

It reports:

1.  A required step with no `before`/`on`/`after` handlers, which would otherwise fail at run time with `OrkaError::HandlerMissing`.
2.  An extractor registered for a step that has no `on::<SData>` handler consuming it.
3.  A `conditional_scopes_for_step` builder that was never finalized, so its scopes were silently discarded.

All problems are collected into a single `OrkaError::ConfigurationError`, not just the first. Calling `validate` yourself is optional — `Orka::register_pipeline` runs it for you — but it is worth doing in a test when you run pipelines directly via `Pipeline::run`.

## 9. Error Handling

`OrkaError` covers framework-level failures:

| Variant | Meaning |
| --- | --- |
| `StepNotFound` | A referenced step was not defined. |
| `HandlerMissing` | A required step has no handlers. |
| `ExtractorFailure` | A sub-context extractor returned an error. |
| `PipelineProviderFailure` | A scoped-pipeline provider failed to yield a pipeline. |
| `TypeMismatch` | A context downcast failed. |
| `HandlerError` | Wraps an external error converted into `OrkaError`. |
| `ConfigurationError` | Validation failure, or no pipeline registered for a type. |
| `NoConditionalScopeMatched` | No scope condition held for a conditional step. |
| `Internal` | Miscellaneous internal failure. |

Applications define their own error type and derive the conversion:

```rust
#[derive(Debug, thiserror::Error)]
enum AppError {
  #[error(transparent)]
  Orka(#[from] OrkaError),
  #[error("database: {0}")]
  Db(#[from] sqlx::Error),
  #[error("payment declined: {0}")]
  Declined(String),
}
```

With that in place, `Pipeline<TData, AppError>` handlers can use `?` on any error convertible into `AppError`, and Orka's own failures arrive as `AppError::Orka`.

`OrkaError` also implements `From<anyhow::Error>`, wrapping it as `HandlerError`. An `anyhow::Error` that already contains an `OrkaError` stays nested rather than being unwrapped; the causal chain is preserved through `#[source]` either way.

`OrkaResult<T, E = OrkaError>` is the crate's result alias, used for `validate` and `register_pipeline`.

### Propagation rules

*   A handler error aborts the run and propagates out of `Pipeline::run`.
*   A failing sub-handler skips the merge — the root context is left untouched.
*   A failing conditional scope propagates unless the step is optional, in which case the error is logged and the pipeline continues.
*   Extractor and provider failures are `OrkaError`s converted into the pipeline's `Err` via `From`.

## 10. Best Practices

**Keep guards short.** Read what you need into locals, drop the guard, then `.await`. A guard held across a suspension point will deadlock the pipeline.

**Prefer merging extractors when results matter.** A detached sub-context silently discards writes, which reads as a no-op bug. Reach for `set_extractor_with_merge` and `.with_merge(..)` unless you specifically want isolation.

**Mark genuinely optional work optional.** A required step with no handlers fails the run; an optional one is skipped. Use `skip_if` for work that is conditional on data rather than on configuration.

**Validate in tests.** Assert `pipeline.validate().is_ok()` for pipelines you run directly, so unfinalized builders and orphaned extractors are caught in CI.

**Give steps stable, descriptive names.** They are the key for every registration and appear in errors and tracing spans.

**Keep sub-contexts small.** Extractors clone, so project the smallest slice a sub-pipeline actually needs.
