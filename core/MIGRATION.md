# Migrating Orka 0.3 → 0.3.1

Cancellation adds a variant to four enums. Everything else in 0.3.1 is additive.

`PipelineResult`, `RunOutcome`, `PlannedAction` and `FanOutItemOutcome` each gain a `Cancelled` variant and are now `#[non_exhaustive]`, so an exhaustive `match` on any of them needs a wildcard arm. The compiler points at every site:

```rust
match pipeline.run(ctx).await? {
  PipelineResult::Completed => ...,
  PipelineResult::Stopped => ...,
  other => ...,                     // add this
}
```

`TraceEventKind` gains `RunCancelled { step, index }` without the attribute, so a `PipelineObserver` that matches every event kind needs one more arm rather than a wildcard.

One change the compiler cannot catch: **a cancelled run is not `Errored`**. Code shaped like

```rust
if matches!(outcome, RunOutcome::Errored { .. }) {
  discard_half_built_release().await?;
}
```

still compiles and silently stops firing for cancelled runs. That distinction is the reason the variant exists (a cancelled deploy has the same half-built state as a failed one, and folding it into `Stopped` would read as a clean early exit), so audit any match on `RunOutcome` that drives cleanup.

Also silent: `FanOutResults::satisfied()` is now false for a cancelled fan-out under every policy including `CollectAll`, and `into_control()` returns `Ok(Stop)` rather than an error. Neither differs from 0.3 unless you call `FanOut::with_cancel`.

See `README.USAGE.md` §13 for the narrative and `API_REFERENCE.md` §10 for signatures.

---

# Migrating Orka 0.2 → 0.3

## 1. `insert_before_step` / `insert_after_step` take `impl AsRef<str>`

Their *new* step name went from `impl Into<String>` to `impl AsRef<str>`, so that a typed step key works on both arguments rather than only the first. String literals and `String` are unaffected. This bites only if you turbofished either call, which the compiler flags, or if you passed something that is `Into<String>` but not `AsRef<str>`, which in practice means `char`:

```rust
pipeline.insert_after_step("a", 'b');    // 0.2
pipeline.insert_after_step("a", "b");    // 0.3
```

Everything else added in 0.3 is additive and needs no action. See `README.USAGE.md` for the narrative and `API_REFERENCE.md` for signatures.

---

# Migrating Orka 0.1 → 0.2

0.2 is a breaking release. The changes are mechanical and the compiler catches nearly all of them; a typical pipeline needs edits in two places (the constructor and the handler bodies).

## At a glance

| 0.1 | 0.2 |
| --- | --- |
| `Pipeline::new(&[("a", false, None), ("b", true, None)])` | `Pipeline::new(["a", "b"])` then `.optional("b")` |
| `pipeline.set_optional("b", true)` | `pipeline.optional("b")` / `pipeline.required("b")` |
| `pipeline.set_skip_condition("a", Some(Arc::new(f)))` | `pipeline.skip_if("a", f)` / `pipeline.clear_skip_condition("a")` |
| `insert_after_step("a", "b", false, None)` | `insert_after_step("a", "b")`, then chain `.optional("b")` |
| `orka.register_pipeline(p);` | `orka.register_pipeline(p)?;`, which validates and returns `OrkaResult<()>` |
| `on::<SData, _, Err>("step", ...)` | `on("step", \|s: ContextData<SData>\| ...)`, annotating the param |
| `AnyPipeline` trait | removed (it was never implemented) |

## Step definitions

Steps are declared by name; optionality and skip conditions are chained afterwards, so the positional `bool` and `Option<SkipCondition>` disappear.

```rust
// 0.1
let mut pipeline = Pipeline::<Ctx, MyError>::new(&[
    ("load", false, None),
    ("notify", true, None),
    ("validate", false, Some(Arc::new(|ctx| ctx.read().already_valid))),
]);

// 0.2
let mut pipeline = Pipeline::<Ctx, MyError>::new(["load", "notify", "validate"]);
pipeline
    .optional("notify")
    .skip_if("validate", |ctx| ctx.read().already_valid);
```

`new` accepts anything iterable of string-likes (`&["a", "b"]`, `["a", "b"]`, `Vec<String>`), so dynamically built step lists no longer need a `Box::leak` dance.

## Handlers

The handler future must now resolve to `Result<PipelineControl, Err>` where `Err` is the pipeline's own error type. Pinning that down is what removes all three papercuts at once: no `Box::pin`, no closure parameter annotation, no turbofish on the `Ok`.

```rust
// 0.1
pipeline.on_root("step", |ctx: ContextData<MyCtx>| Box::pin(async move {
    ctx.write().count += 1;
    Ok::<_, MyError>(PipelineControl::Continue)
}));

// 0.2
pipeline.on_root("step", |ctx| async move {
    ctx.write().count += 1;
    Ok(PipelineControl::Continue)
});
```

Handlers that produced a *different* error type and relied on the old `Into<Err>` widening now need an explicit `?` or `.map_err(Into::into)?`. Because `?` converts through `From` as before, most such handlers already compile unchanged.

Existing `Box::pin(...)` handlers still satisfy the new bound, so you can migrate the constructor first and the handler bodies at your leisure.

## Chaining

Every registration and configuration method returns `&mut Self`:

```rust
pipeline
    .optional("notify")
    .on_root("load", |ctx| async move { Ok(PipelineControl::Continue) })
    .on_root("validate", |ctx| async move { Ok(PipelineControl::Continue) });
```

## Sub-context handlers

`on` lost its error generic. Annotate the closure parameter: that is what tells Orka which `SData` you mean.

```rust
// 0.1
pipeline.on::<CustomerInfo, _, MyError>("validate", |s: ContextData<CustomerInfo>| Box::pin(async move {
    Ok(PipelineControl::Continue)
}));

// 0.2
pipeline.on("validate", |s: ContextData<CustomerInfo>| async move {
    Ok(PipelineControl::Continue)
});
```

## Registry

`register_pipeline` validates the pipeline and returns `OrkaResult<()>`:

```rust
orka.register_pipeline(pipeline)?;
```

Setup mistakes that used to surface on the first run (a required step with no handlers, for instance) now surface at registration.

## New in 0.2

Nothing below is required to migrate, but each removes a workaround that 0.1 forced.

- **`set_extractor_with_merge(step, extractor, merge_fn)`**: plain `set_extractor` hands the sub-handler a *detached* copy, so its writes are discarded. This variant folds the sub-context back into the parent when the handler succeeds.
- **`.with_merge(|main, sub| ...)`** on conditional scopes: the same, for scoped pipelines. Previously a scope could not report anything back, which forced smuggling a `ContextData` through the parent context.
- **`ContextData::project(|d| d.field.clone())`**: the idiomatic way to write an extractor.
- **`Pipeline::validate() -> OrkaResult<()>`**: reports required steps with no handlers, extractors nothing consumes, and conditional builders that were never finalized. It collects every problem rather than stopping at the first.
- **`orka::prelude`**: the common imports in one line.
- **Bug fix:** conditional scopes now *append* their handler instead of replacing it, so an `on_root` handler registered on the same step still runs. In 0.1 it was silently dropped.
