//! Proves `orka::prelude` is sufficient for the everyday build/configure/run/inspect path.
//!
//! Every type used to drive the pipeline here comes from the prelude glob; the only other
//! import is the error fixture. If something this path needs ever drops out of the
//! prelude, this file stops compiling. Without a test like this the prelude silently went
//! stale twice: it never picked up `PipelineRunner`, nor the `resolve_plan` result types.

use orka::prelude::*;
use orka::test_util::TestError; // fixture only: a Clone + PartialEq error with From<OrkaError>
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
struct Ctx {
  drain_enabled: bool,
  log: Vec<String>,
}

fn build() -> Pipeline<Ctx, TestError> {
  let mut p: Pipeline<Ctx, TestError> = Pipeline::new(["prepare", "drain", "install"]);
  p.on_root("prepare", |ctx| async move {
    ctx.write().log.push("prepare".into());
    Ok(PipelineControl::Continue)
  })
  .on_root("drain", |ctx| async move {
    ctx.write().log.push("drain".into());
    Ok(PipelineControl::Continue)
  })
  .on_root("install", |ctx| async move {
    ctx.write().log.push("install".into());
    Ok(PipelineControl::Continue)
  })
  .skip_if_labeled("drain", "drain disabled by config", |ctx| !ctx.read().drain_enabled)
  .must_precede("prepare", "install")
  .on_finish(|_ctx, _outcome: RunOutcome| async { Ok(()) });
  p
}

#[tokio::test]
async fn prelude_covers_build_configure_run_and_inspect() {
  let pipeline = build();

  // Validation and introspection.
  let validated: OrkaResult<()> = pipeline.validate();
  assert!(validated.is_ok());
  assert!(pipeline.has_handlers("drain", StepPhase::On));
  assert_eq!(pipeline.step_names(), vec!["prepare", "drain", "install"]);

  // Dry run, including the labeled skip reason.
  let ctx = ContextData::new(Ctx::default());
  let plan: Vec<StepPlan> = pipeline.resolve_plan(&ctx);
  match &plan[1].action {
    PlannedAction::Skip(SkipReason::SkipCondition { label }) => {
      assert_eq!(label.as_deref(), Some("drain disabled by config"));
    }
    other => panic!("expected a labeled skip, got {:?}", other),
  }

  // Run, with the outcome alongside the result.
  let (result, outcome) = pipeline.run_with_outcome(ctx.clone()).await;
  assert_eq!(result.unwrap(), PipelineResult::Completed);
  assert_eq!(outcome, RunOutcome::Completed);
  assert_eq!(ctx.read().log, vec!["prepare", "install"]);

  // Run boundary as a trait object, then through the registry.
  let runner: Arc<dyn PipelineRunner<Ctx, TestError>> = Arc::new(build());
  let orka: Orka<TestError> = Orka::new();
  orka.register_runner(runner);

  let ctx2 = ContextData::new(Ctx {
    drain_enabled: true,
    ..Ctx::default()
  });
  assert_eq!(orka.run(ctx2.clone()).await.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx2.read().log, vec!["prepare", "drain", "install"]);
}
