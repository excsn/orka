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
11. [Run-Level Cleanup and Observation](#11-run-level-cleanup-and-observation)
12. [Testing Your Pipelines](#12-testing-your-pipelines)

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

This brings in `Pipeline`, `PipelineRunner`, `Orka`, `ContextData`, `PipelineControl`, `PipelineResult`, `StepDef`, `SkipCondition`, `OrkaError`, `OrkaResult`, plus the types you cannot avoid naming when calling a `Pipeline` method: `RunOutcome` (for `on_finish` and `run_with_outcome`), `StepPlan`, `PlannedAction` and `SkipReason` (for `resolve_plan`), and `StepPhase` (for `has_handlers`).

Two clusters stay at the crate root, since you reach for them deliberately rather than meeting them in a signature: advanced items (`Handler`, `ContextDataExtractorImpl`, `AnyContextDataExtractor`, the pipeline providers, the conditional-scope builders) and observability (`TraceCollector`, `PipelineObserver`, `CompositeObserver`, `TraceEvent`, `TraceEventKind`, `HandlerOutcome`, `RunTrace`).

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

### Typed step keys

Every step-name parameter in the API takes `impl AsRef<str>`, not `&str`. String literals work as shown throughout this guide, but on a pipeline of any size it is worth naming the steps once and letting the compiler carry them:

```rust
#[derive(Clone, Copy)]
enum Step { Prepare, Drain, Install }

impl Step {
  const ALL: [Step; 3] = [Step::Prepare, Step::Drain, Step::Install];
}

impl AsRef<str> for Step {
  fn as_ref(&self) -> &str {
    match self {
      Step::Prepare => "prepare",
      Step::Drain => "drain",
      Step::Install => "install",
    }
  }
}

let mut pipeline = Pipeline::<Ctx, MyError>::new(Step::ALL);
pipeline
  .on_root(Step::Prepare, prepare)
  .skip_if_labeled(Step::Drain, "drain disabled by config", |ctx| !ctx.read().drain_enabled)
  .must_precede(Step::Prepare, Step::Install);
```

A typo is now a compile error rather than a runtime panic, a rename is a refactor rather than a search, and the step list autocompletes. This works at every site that names a step: registration, `skip_if`, `must_precede`, the overrides, `run_step`, `has_handlers`, and the inserts. Typed keys and plain strings mix freely, so adopting this is incremental.

## 3. Steps: Optionality, Skipping, and Mutation

Optionality and skip conditions are chained after construction:

```rust
pipeline
  .optional("notify")                                  // may have no handlers; scope errors swallowed
  .required("audit")                                   // back to the default
  .skip_if("validate", |ctx| ctx.read().already_valid) // evaluated at run time
  .skip_if_labeled("drain", "drain disabled by config", |ctx| !ctx.read().drain_enabled)
  .clear_skip_condition("validate");
```

`skip_if_labeled` is `skip_if` plus a human-readable label; the label is carried into `resolve_plan` output and `StepSkipped` trace events, so previews and skip-matrix tests say *why* a step skips ("drain disabled by config") instead of showing an anonymous condition. Re-registering with plain `skip_if` clears a stale label.

Ordering invariants between steps can be declared once and checked at setup time by `validate()` instead of failing as a mid-run panic:

```rust
pipeline
  .must_precede("drain", "stop_unit")
  .must_precede_all("unpack", ["base_labels", "load_spec", "secrets"]);
```

`validate()` reports any pair the actual step order violates. Deliberately, `remove_step` does **not** clean these constraints: removing a step that others declared a dependency on leaves a dangling constraint, and `validate()` fails loudly, which is exactly the edit this feature exists to catch.

### Resource dependencies

Most orderings exist for a reason: a later step reads something an earlier step wrote. Saying that directly is better than encoding it as ordering pairs, because the reason survives in the code and `validate()` gains a check the pairs cannot express:

```rust
pipeline
  .produces("unpack", "release")
  .consumed_by("release", ["base_labels", "load_spec", "secrets", "ownership"]);
```

Two lines per resource replace one pair per consumer, and `validate()` now reports three things: a consumer that runs before its producer, a declaration left dangling by `remove_step`, and, most valuably, **a resource that is consumed but that no step produces**. That last one is a real bug class with no other early warning: rename or delete the producing step and every consumer keeps compiling, then panics at `.expect("set by the unpack step")` in the middle of a run. It is reported once per resource, listing every affected consumer.

A resource may have more than one producer, in which case every consumer must follow all of them, since which producer actually runs is not knowable at setup time. Resource names are `AsRef<str>` like step names, so a `Res` enum keeps both sides typo-proof.

Use `must_precede` / `must_precede_all` for orderings that are about effects rather than data ("drain before stop-unit"), and `produces` / `consumed_by` whenever a value is being threaded from one step to later ones.

Declaring the dependency is half of it. The consuming step still has to read the value, and `ctx.require` does that without a panic:

```rust
let spec = ctx.require(Res::AppSpec, |c| c.app_spec.clone())?;
```

That replaces the `.expect("app_spec set by load-spec step")` that would otherwise sit at every consuming site, restating in a string what the declaration already says. The gain is not tidiness but cleanup: a panic unwinds past the run's `on_finish` ring and past resource release, and whatever does drop then drops front to back rather than in reverse; inside a spawned fan-out it degrades further into `FanOutBranchLost`, which reads as an infrastructure fault rather than a pipeline bug. A handled `OrkaError::ResourceMissing` leaves all of that intact, and `run_with_outcome` names the step that was reading.

Note that nothing checks the name you require against the name you declared, because a context does not know which step is reading it. What couples the two is using the same key in both places, so a typed `Res` enum makes a rename move both at once.

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

`with_ref` and `with_mut` make that rule structural instead of a convention. They run a synchronous closure under the lock and release the guard before returning, so the lock cannot reach a suspension point no matter how the handler is written:

```rust
.on_root("fetch", |ctx| async move {
  let url = ctx.with_ref(|c| c.url.clone());
  let body = http::get(&url).await?;
  ctx.with_mut(|c| c.body = body);
  Ok(PipelineControl::Continue)
})
```

Both return the closure's value, which is what makes non-`Clone` intermediates comfortable. A value one step produces and a later step mutates does not need to be `Arc`'d (and so does not need an `Arc::try_unwrap` / mutate / re-wrap dance): `with_mut` hands out a scoped `&mut` to the field, and returns a value when you want ownership back.

```rust
ctx.with_mut(|c| c.specs.as_mut().expect("set by the parse step").entries.push(entry));
let specs = ctx.with_mut(|c| c.specs.take());   // ownership out, one line
```

An `Arc` inside the context is a genuinely different situation: if a field is `Arc<U>` because something outside the context holds it too, mutating `U` in place is `Arc::get_mut`'s job. But if it is `Arc`'d only so readers can see a non-`Clone` value, `map_read` already borrows a field without cloning, and the `Arc` may not be needed at all.

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

### Fan-out: running one pipeline over many items

Conditional scopes are one-of-N: the first matching scope runs. `FanOut` is the all-of-N counterpart, for the case where a step has a *collection* of work and one sub-pipeline to run over each item. It is a combinator rather than a builder, so it is called from inside an ordinary handler:

```rust
pipeline.on_root("deploy_all", move |ctx| {
  let targets = target_pipeline.clone();
  async move {
    let results = FanOut::new(targets)
      .max_concurrent(8)
      .policy(FanOutPolicy::CollectAll)
      .run(ctx.with_ref(|c| c.placements.clone()))
      .await;

    ctx.with_mut(|c| c.deployed = results.cloned_oks());
    results.into_control()
  }
});
```

Each branch is a full `Pipeline::run`, so every item gets its own `on_finish` ring, its own resource-bag release, and its own run id in a trace.

**Results are never discarded.** `run` always returns every branch's outcome, including when the policy is unsatisfied, because partial success is data rather than an error condition. That is what makes "three of five deployed" answerable: `results.succeeded()`, `results.oks()` for the contexts that worked, and `results.errors()` yielding each failure with its **input index** so you can say *which* item failed. Per-item errors stay typed, not flattened into a string, and results come back in input order regardless of completion order.

**Policies** decide only whether the fan-out is *satisfied*:

| Policy | Satisfied when |
|---|---|
| `CollectAll` (default) | always; failures are still in the results |
| `RequireAll` | every branch ran without error |
| `RequireAtLeast(n)` | at least `n` branches ran without error |
| `FailFast` | no branch failed |

`custom_policy(|results| ...)` covers what the four cannot ("satisfied if the primary region succeeded"). `into_control()` turns the verdict into a handler's return: `Ok(Continue)` when satisfied, otherwise the first branch's typed error, or `OrkaError::FanOutPolicyUnmet` when the policy is unmet without any branch having failed.

`FailFast` is the only policy that acts before every branch settles, and it **stops starting new branches rather than cancelling in-flight ones**. Cancelling would mean dropping a running pipeline mid-flight, whose `on_finish` handlers would never fire and whose resources would release late, which is precisely the cleanup this engine exists to make reliable. Branches that never started are reported as `NotStarted` and have run no code at all.

**By default, concurrency here is cooperative rather than parallel.** orka depends on no async runtime, so branches are polled on the caller's task and make progress while each other awaits. That fits I/O-bound work (network calls, uploads, waiting on a remote event), which is the usual shape of per-item fan-out. A branch that blocks the thread, whether by a synchronous file read, a long CPU stretch, or a lock guard held across a yield point, stalls its siblings.

### Spawning branches as real tasks

When you do want parallelism, hand the fan-out a `TaskSpawner`. Enable the `tokio` feature and the shipped one needs no glue:

```rust
let results = FanOut::new(targets)
  .spawner(Arc::new(TokioSpawner))
  .max_concurrent(8)
  .run(placements)
  .await;
```

Any other runtime is a five-line impl: spawn the task, return a handle that resolves when it finishes (including on panic or abort, so orka reports rather than hangs).

A branch spawns on its **first poll**, not when the fan-out is built, so `max_concurrent` and fail-fast still govern how many tasks exist and which items ever become tasks at all. Branches reported as `NotStarted` were never spawned.

Two behaviours change once a spawner is in play:

*   **A panicking branch is contained.** Cooperatively, a branch that panics unwinds the whole fan-out and your caller with it. Spawned, the runtime catches it, and orka reports that branch as failed with `OrkaError::FanOutBranchLost` while its siblings finish normally.
*   **Dropping the fan-out no longer cancels in-flight work.** Cooperative branches are owned by the fan-out future and stop when it is dropped; spawned ones belong to the runtime and keep running detached.

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

### Per-step timeouts

Orka imposes no timeouts and offers no `step_timeout`, because it depends on no async runtime: it has no timer and cannot wake a handler that is hung. Bounding a step's time is therefore the handler's own job, using whatever its runtime provides.

What orka does provide is the reporting. Returning `OrkaError::StepTimedOut { step_name, after }` gives every hand-rolled timeout a single shape, and carries the step's name into `run_with_outcome` and the trace, which is exactly what an anonymous `tokio::time::timeout` error loses unless each call site remembers to encode it. The run then reports `Errored { step: "install", .. }` rather than an unattributed failure, so an operator-facing message can name the step that overran.

With the `tokio` feature, `timed` collapses the match-and-map that otherwise repeats at every such call site:

```rust
pipeline.on_root(Step::AwaitArtifact, |ctx| async move {
  let (rx, budget) = ctx.with_ref(|c| (c.archive_ready_rx.clone(), c.artifact_timeout));

  // Two independent failure modes, so two unwraps: the timeout, then the receive.
  let msg = timed(Step::AwaitArtifact, budget, rx.recv()).await??;

  ctx.with_mut(|c| c.artifact_id = msg.artifact_id);
  Ok(PipelineControl::Continue)
});
```

The budget bounds **that await only**, not the rest of the handler, which is usually what you want: the call that may never return is a specific one, and keeping the timeout local means you can still react to it (publish a message, fall back, retry) rather than only fail. Where no wrapper fits at all, such as a poll loop with its own deadline or a callee that takes the timeout as a parameter, returning `StepTimedOut` by hand still buys the uniform reporting.

One caveat worth stating: a timeout drops the handler future, so whatever it had in flight is abandoned mid-way. The run itself continues to its exit, so the `on_finish` ring still fires and the resource bag still releases; it is only the step's own partial work that is lost. That is inherent to timeouts, and it is why fan-out's `FailFast` drains instead of cancelling: a timeout is a hard bound, whereas fail-fast is an optimisation.

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

## 11. Run-Level Cleanup and Observation

### `on_finish`: an async "finally" for the whole run

`after_root` is per-step and only fires if its step runs, so it cannot express "no matter how this run ends, do X". `on_finish` can: it registers a run-level finish handler, awaited on every exit of a full `run()` (completed, stopped by a handler, or failed, including the missing-handler configuration error), with the final shared context and the run's `RunOutcome`.

```rust
pipeline.on_finish(|ctx, outcome| async move {
  // The classic shape: release a resource a step acquired, whether or not later steps failed.
  if let Some(drain_id) = ctx.write().drain_id.take() {
    drainer.restore(drain_id).await;
  }
  Ok(())
});
```

Multiple finish handlers run in registration order, and all of them run even if one fails. The error policy: on a run that returned `Ok` (Completed or Stopped), the first finish-handler error becomes the run's error, because a cleanup failure on a success path must surface. On an already-failed run, finish-handler errors are logged and the original error is returned, because cleanup must not mask the real failure.

The partial runners (`run_step`, `run_from`, `run_until`) and `resolve_plan` never fire finish handlers; use `run()` when you want finish semantics.

### Run-scoped resources

Some things a run acquires are not part of its data model: a mutex guard held for the duration of a build, a `TempDir` the steps write into, an open file handle. Carrying those as `Option<T>` fields on the context works, but the context type then claims they are workflow data, and releasing them becomes a hand-written `take()` in a finish handler, with the drop ordering done by hand. Stash them in the context's resource bag instead:

```rust
.on_root("acquire", |ctx| async move {
  let temp = TempDir::new()?;
  ctx.with_mut(|c| c.build_dir = temp.path().to_path_buf());  // the path is data
  ctx.resources().put(lock_guard).put(temp);                  // the handles are not
  Ok(PipelineControl::Continue)
})
```

Everything stashed is dropped at the end of a full `run()`, in reverse order of insertion, **after** the `on_finish` handlers. That ordering is deliberate: a finalizer can still copy artifacts out of the temp dir, or write a last record under the lock, before either is released. Release is unconditional, like any `Drop`, so a failed run releases exactly as a successful one does.

A resource that also carries a usable value stays reachable without duplicating it into the context:

```rust
let path = ctx.resources().with(|t: &TempDir| t.path().to_path_buf());
```

`with` borrows the most recently stashed value of the requested type, so it returns `None` if nothing of that type is held. The bag's lock is held while the closure runs, so don't call back into the same bag from inside it.

That lock is also why `with` suits resources a run merely *holds*. One it *operates on*, such as a stream sender that chunks are awaited into, needs `&mut` across suspension points, and for that there is `take_guard`:

```rust
let mut sender = ctx.resources().take_guard::<StreamSender>().expect("stashed at open");
for chunk in chunks {
  sender.send(chunk).await?;   // an owned guard, not a lock guard, so awaits are fine
}
```

The guard returns the resource to the bag when it drops, and because that happens in `Drop` no path skips it. An early `?`, a panic, or a handler cancelled by a timeout all put it back, so it is still released at the run's defined point rather than inside a dropped future. Use plain `take` only when the resource genuinely is not coming back, and `keep()` on a guard to opt out of the return.

Two boundaries are worth knowing. Rust has no async `Drop`, so this is for resources whose cleanup is synchronous and quick; anything that must be awaited (committing a transaction, draining a connection) belongs in an `on_finish` handler. And only a full `run()` releases the bag: the partial runners leave it alone, exactly as they leave the finish ring alone, so a `run_step` test can stash something and still inspect it. Nothing leaks either way, since whatever is still held drops when the last `ContextData` handle for that context drops.

### Observing execution

Attach a `PipelineObserver` and every run reports its progress as `TraceEvent`s: steps started, skipped (with the reason), completed, per-handler outcomes, conditional scope selection, finish handlers, and the final outcome. `TraceCollector` is the batteries-included observer that buffers events and answers queries (`completed_steps()`, `step_skipped(..)`, `last_outcome()`); a custom `impl PipelineObserver` can instead stream to metrics or logs with no buffering.

```rust
let trace = TraceCollector::new();
pipeline.set_tracer(trace.clone()); // &self: works on a pipeline behind an Arc
pipeline.run(ctx).await?;
assert!(trace.step_completed("charge"));
```

Attachment is `&self` (the slot is interior-mutable), so it works on a pipeline already registered in an `Orka` and obtained via `orka.pipeline::<MyCtx, MyErr>()`. The observer is snapshotted at run start: attaching while a run is in flight misses that run and catches the next one. Events are tagged with a per-run `run_id`; concurrent runs of a shared pipeline interleave in one collector, so scope queries with `trace.for_run(id)`. A `TraceCollector` is an accumulating log, so prefer a streaming observer (or periodic `clear()`) if you attach one in production.

The slot holds a single observer; when a production bridge and a diagnostic collector must coexist, compose them with `CompositeObserver` (push each `Arc<dyn PipelineObserver>`, attach the composite) instead of displacing one another.

Attaching binds an observer to the *pipeline*, which is the wrong scope for tracing one production run: a registered pipeline shared by concurrent runs reports all of them into the same collector, and you cannot filter to your own, because the run id is allocated inside `run` and there is nothing to hand to `for_run`. Pass the observer to the call instead:

```rust
let trace = TraceCollector::new();
let (result, outcome) = pipeline.run_with_observer(ctx, Arc::new(trace.clone())).await;
```

An attached observer is not displaced; both see every event. A scoped observer is also **inherited by runs started from inside this one**, so fan-out branches and conditional sub-pipelines report into the same collector and one trace covers the whole call tree. That inheritance applies only to scoped observers: an attached one stays bound to its own pipeline's runs. Note it gives you isolation ("these events are mine") rather than hierarchy: branch runs carry their own run ids, and nothing yet records which parent step spawned them.

### Failed-step identity: `run_with_outcome`

The plain `Err` from `run()` cannot carry the failing step's name without changing your error type. `run_with_outcome(ctx)` returns `(Result<PipelineResult, Err>, RunOutcome)`, where a failure's outcome is `Errored { step, message }`; `Orka::run_with_outcome` passes it through the registry. This is what a job shell wants for operator-facing reporting ("deploy failed at 'install-start'") without attaching an observer. A finish-handler failure on an otherwise-Ok run is attributed to the step name `"on_finish"`; mocks and middleware that do not override `PipelineRunner::run_with_outcome` report an empty step (they cannot attribute failures).

### Previewing a run: `resolve_plan`

`resolve_plan(&ctx)` evaluates every step's `skip_if` predicate plus the handler-presence checks against a seeded context and reports what a run would do (`Run`, `Skip(reason)`, or `FailMissingHandlers`), executing nothing. Skips carry the `skip_if_labeled` label, so the output is self-explaining ("skip: drain disabled by config"). Predicates are evaluated against that one static context, so step-to-step data flow is not simulated; it is a preview, and a perfect fit for table tests over a skip matrix.

## 12. Testing Your Pipelines

Enable the `test-util` feature in your dev-dependencies to get `orka::test_util` (canned handlers, `MockPipeline`, `ExecutionCounter`, trace assertions):

```toml
[dependencies]
orka = "0.3"

[dev-dependencies]
orka = { version = "0.3", features = ["test-util"] }
```

### The registry-native recipe

A fresh `Orka` per test is cheap and self-contained, so the registry itself is the test scope. Call the same production registration function tests and prod both use, then reach the registered pipeline through `orka.pipeline()`; there is no parallel dummy pipeline to keep in sync:

```rust
let orka = build_registry()?; // your app's real wiring

// Observe the real registered pipeline:
let trace = TraceCollector::new();
let p = orka.pipeline::<MyCtx, MyErr>().unwrap();
p.set_tracer(trace.clone());

// Table-test the skip logic without executing anything:
let plan = p.resolve_plan(&seeded_ctx);

// Or run for real, through the same entry point production uses:
orka.run(ctx.clone()).await?;
orka::test_util::assert_steps_skipped(&trace, &["drain", "stop-existing"]);
```

### Stubbing and error injection

Handler overrides are `&mut self`, so a live registered pipeline cannot be mutated (correct: it may be running concurrently). The pattern is build, mutate, register: build via your production build function, override while you still hold it `&mut`, then register it into a fresh `Orka` (registering over an existing entry replaces it, so this also swaps in instrumented variants).

*   `replace_before_root` / `replace_on_root` / `replace_after_root` surgically replace one phase's handlers; `clear_before` / `clear_on` / `clear_after` empty one phase.
*   `fail_at(step, make_err)` (from `test_util::PipelineTestExt`) forces a failure at a step, for exercising error paths and `on_finish` backstops.
*   `stub_step(step)` neutralizes a whole step, including any conditional master handler and extractor, leaving a single Continue handler so `validate()` stays green.
*   `noop_pipeline(names)` builds a continue-only pipeline with your real step names, the canned shape for structural and skip-condition tests.

### Step isolation

`run_step("unpack", ctx)` executes exactly one step's phases against a seeded context, so a test can assert one handler's context transform without running the whole pipeline. `run_from` and `run_until` run inclusive ranges. These are inspection tools: they respect `skip_if`, but emit no `RunStarted`/`RunFinished` events and never fire `on_finish` handlers.

### Faking the whole pipeline

Where application code only needs to *execute* a pipeline, depend on the `PipelineRunner` trait instead of the concrete type, or go through the registry. `MockPipeline` is a canned runner: base behavior from its constructor (`completed()`, `stopped()`, `failing(..)`, or `from_fn(..)` for full control), one-shot responses queued FIFO via `then_completed()/then_stopped()/then_error(..)`, plus `run_count()` and `contexts()` for inspection.

```rust
let mut mock = MockPipeline::<CheckoutCtx, AppError>::completed();
mock.then_stopped(); // first run: Stopped; after that: Completed
orka.register_runner::<CheckoutCtx, AppError>(Arc::new(mock));
// The handler under test calls orka.run(..) exactly as in production.
```

`PipelineRunner` is a production seam too: retry, timeout, or logging middleware is just an implementation that wraps another `Arc<dyn PipelineRunner>`, registered via `register_runner`. (`orka.pipeline()` honestly returns `None` for runner-only registrations, since there is no concrete pipeline to hand back.)

### Injection seams for scopes and extractors

`add_scope_with_provider(Arc<dyn PipelineProvider<..>>, extractor)` is the trait-object generalization of `add_static_scope`/`add_dynamic_scope`, so a test can inject a recording or canned provider. `set_extractor_impl(step, Arc<dyn AnyContextDataExtractor<..>>)` is the same seam behind `set_extractor`/`set_extractor_with_merge`. For counting invocations of providers, extractors, or handlers, clone a `test_util::ExecutionCounter` into the closure and assert locally; no global state, no serial tests.
