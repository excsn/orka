// tests/registry_tests.rs
mod common;

use common::*;
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl, PipelineResult};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegistryContextAlpha {
  val: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RegistryContextBeta {
  num: i32,
}

#[tokio::test]
async fn test_registry_run_correct_pipeline() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(["alpha_task"]);
  p_alpha.on_root("alpha_task", |ctx| async move {
    ctx.write().val = "alpha_processed".to_string();
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p_alpha).unwrap();

  let mut p_beta = Pipeline::<RegistryContextBeta, TestError>::new(["beta_task"]);
  p_beta.on_root("beta_task", |ctx| async move {
    ctx.write().num = 100;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p_beta).unwrap();

  // Run Alpha
  let ctx_alpha = ContextData::new(RegistryContextAlpha::default());
  let res_alpha = orka_registry.run(ctx_alpha.clone()).await;
  assert!(res_alpha.is_ok());
  assert_eq!(res_alpha.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx_alpha.read().val, "alpha_processed");

  // Run Beta
  let ctx_beta = ContextData::new(RegistryContextBeta::default());
  let res_beta = orka_registry.run(ctx_beta.clone()).await;
  assert!(res_beta.is_ok());
  assert_eq!(res_beta.unwrap(), PipelineResult::Completed);
  assert_eq!(ctx_beta.read().num, 100);
}

#[tokio::test]
async fn test_registry_pipeline_not_found() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  #[derive(Clone, Debug, Default)]
  struct UnregisteredContext;

  let ctx_unregistered = ContextData::new(UnregisteredContext);
  let result = orka_registry.run(ctx_unregistered).await;

  assert!(result.is_err());
  if let Err(TestError::Orka(s)) = result {
    assert!(s.contains("ConfigurationError"));
    assert!(s.contains("No pipeline registered"));
    assert!(s.contains("UnregisteredContext"));
  } else {
    panic!(
      "Expected OrkaError(ConfigurationError) for unregistered pipeline, got {:?}",
      result
    );
  }
}

#[tokio::test]
async fn test_registry_pipeline_itself_errors() {
  setup_tracing();
  let orka_registry = Orka::<TestError>::new();

  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(["alpha_fail"]);
  p_alpha.on_root("alpha_fail", |_ctx| async move {
    Err(TestError::Handler("Alpha pipeline failed".to_string()))
  });
  orka_registry.register_pipeline(p_alpha).unwrap();

  let ctx_alpha = ContextData::new(RegistryContextAlpha::default());
  let res_alpha = orka_registry.run(ctx_alpha.clone()).await;

  assert!(res_alpha.is_err());
  assert_eq!(
    res_alpha.err().unwrap(),
    TestError::Handler("Alpha pipeline failed".to_string())
  );
}

#[tokio::test]
async fn test_registry_with_orka_error_default() {
  setup_tracing();
  let orka_registry = Orka::<OrkaError>::new_default();

  #[derive(Clone, Debug, Default)]
  struct SimpleCtx {
    count: i32,
  }

  // The pipeline's handler error type must match the registry's application error type.
  let mut pipeline = Pipeline::<SimpleCtx, OrkaError>::new(["task"]);
  pipeline.on_root("task", |ctx| async move {
    ctx.write().count = 1;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(pipeline).unwrap();

  let ctx = ContextData::new(SimpleCtx::default());
  let result = orka_registry.run(ctx.clone()).await;
  assert!(result.is_ok());
  assert_eq!(ctx.read().count, 1);
}
