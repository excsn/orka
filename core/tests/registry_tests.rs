// tests/registry_tests.rs
mod common;

use common::*;
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl, PipelineResult};

#[derive(Clone, Debug, Default, PartialEq, Eq)] // PartialEq, Eq for assertion
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
  let orka_registry = Orka::<TestError>::new(); // Registry uses TestError

  // Pipeline Alpha
  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(&[("alpha_task", false, None)]);
  p_alpha.on_root("alpha_task", |ctx: ContextData<RegistryContextAlpha>| {
    Box::pin(async move {
      ctx.write().val = "alpha_processed".to_string();
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });
  orka_registry.register_pipeline(p_alpha);

  // Pipeline Beta
  let mut p_beta = Pipeline::<RegistryContextBeta, TestError>::new(&[("beta_task", false, None)]);
  p_beta.on_root("beta_task", |ctx: ContextData<RegistryContextBeta>| {
    Box::pin(async move {
      ctx.write().num = 100;
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });
  orka_registry.register_pipeline(p_beta);

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
  let orka_registry = Orka::<TestError>::new(); // Registry uses TestError
                                                // No pipelines registered

  #[derive(Clone, Debug, Default)]
  struct UnregisteredContext;

  let ctx_unregistered = ContextData::new(UnregisteredContext::default());
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

  let mut p_alpha = Pipeline::<RegistryContextAlpha, TestError>::new(&[("alpha_fail", false, None)]);
  p_alpha.on_root("alpha_fail", |_ctx: ContextData<RegistryContextAlpha>| {
    Box::pin(async move { Err(TestError::Handler("Alpha pipeline failed".to_string())) })
  });
  orka_registry.register_pipeline(p_alpha);

  let ctx_alpha = ContextData::new(RegistryContextAlpha::default());
  let res_alpha = orka_registry.run(ctx_alpha.clone()).await;

  assert!(res_alpha.is_err());
  assert_eq!(
    res_alpha.err().unwrap(),
    TestError::Handler("Alpha pipeline failed".to_string())
  );
}

// Test with Orka<OrkaError> directly
#[tokio::test]
async fn test_registry_with_orka_error_default() {
  setup_tracing();
  let orka_registry = Orka::<OrkaError>::new_default(); // Uses OrkaError as ApplicationError

  #[derive(Clone, Debug, Default)]
  struct SimpleCtx {
    count: i32,
  }

  // Pipeline must also use OrkaError as its handler error type
  let mut pipeline = Pipeline::<SimpleCtx, OrkaError>::new(&[("task", false, None)]);
  pipeline.on_root("task", |ctx: ContextData<SimpleCtx>| {
    Box::pin(async move {
      ctx.write().count = 1;
      Ok::<PipelineControl, OrkaError>(PipelineControl::Continue)
    })
  });
  orka_registry.register_pipeline(pipeline);

  let ctx = ContextData::new(SimpleCtx::default());
  let result = orka_registry.run(ctx.clone()).await;
  assert!(result.is_ok());
  assert_eq!(ctx.read().count, 1);
}
