use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use orka::{ContextData, Orka, OrkaError, Pipeline, PipelineControl};
use std::sync::Arc;
use tokio::runtime::Runtime;

// --- Common Test/Benchmark Contexts and Error ---
#[derive(Clone, Debug, Default)]
struct BenchContext {
  counter: u64,
}

#[derive(Clone, Debug, Default)]
struct ScopedBenchContext;

type BenchError = OrkaError;

/// CPU-bound work, no suspension point.
fn create_sync_increment_handler(iterations: u64) -> orka::Handler<BenchContext, BenchError> {
  Box::new(move |ctx: ContextData<BenchContext>| {
    Box::pin(async move {
      let mut data = ctx.write();
      for _i in 0..iterations {
        data.counter = data.counter.wrapping_add(1);
      }
      Ok(PipelineControl::Continue)
    })
  })
}

/// Simulated I/O: suspends, then does minimal work.
fn create_async_io_handler(delay_micros: u64) -> orka::Handler<BenchContext, BenchError> {
  Box::new(move |ctx: ContextData<BenchContext>| {
    Box::pin(async move {
      if delay_micros > 0 {
        tokio::time::sleep(std::time::Duration::from_micros(delay_micros)).await;
      }
      ctx.write().counter += 1;
      Ok(PipelineControl::Continue)
    })
  })
}

fn bench_simple_pipeline_sync_handlers(c: &mut Criterion) {
  let mut group = c.benchmark_group("SimplePipelineSync");
  let rt = Runtime::new().unwrap();

  for num_steps_val in [1, 5, 10].iter() {
    for handler_iterations in [1, 10, 100].iter() {
      let step_names: Vec<String> = (0..*num_steps_val).map(|i| format!("step_{}", i)).collect();
      let mut pipeline = Pipeline::<BenchContext, BenchError>::new(&step_names);

      for i in 0..*num_steps_val {
        let step_name = format!("step_{}", i);
        pipeline.on_root(&step_name, create_sync_increment_handler(*handler_iterations));
      }
      let pipeline_arc = Arc::new(pipeline);

      group.throughput(Throughput::Elements(*num_steps_val * *handler_iterations));
      group.bench_with_input(
        BenchmarkId::new(
          format!("{}steps_{}iter", num_steps_val, handler_iterations),
          *num_steps_val * *handler_iterations,
        ),
        &(*num_steps_val, *handler_iterations),
        |b, &(_num_steps_param, _iter_param)| {
          b.to_async(&rt).iter_batched(
            || ContextData::new(BenchContext::default()),
            |ctx| {
              let p_clone = pipeline_arc.clone();
              async move { p_clone.run(ctx).await.unwrap() }
            },
            criterion::BatchSize::SmallInput,
          );
        },
      );
    }
  }
  group.finish();
}

fn bench_simple_pipeline_async_handlers(c: &mut Criterion) {
  let mut group = c.benchmark_group("SimplePipelineAsyncIO");
  let rt = Runtime::new().unwrap();

  for num_steps_val in [1, 5, 10].iter() {
    for delay_us in [0, 10, 100].iter() {
      let step_names: Vec<String> = (0..*num_steps_val).map(|i| format!("step_{}", i)).collect();
      let mut pipeline = Pipeline::<BenchContext, BenchError>::new(&step_names);
      for i in 0..*num_steps_val {
        let step_name = format!("step_{}", i);
        pipeline.on_root(&step_name, create_async_io_handler(*delay_us));
      }
      let pipeline_arc = Arc::new(pipeline);

      group.throughput(Throughput::Elements(*num_steps_val as u64));
      group.bench_with_input(
        BenchmarkId::new(format!("{}steps_{}us_delay", num_steps_val, delay_us), *delay_us),
        delay_us,
        |b, &_delay_param| {
          b.to_async(&rt).iter_batched(
            || ContextData::new(BenchContext::default()),
            |ctx| {
              let p_clone = pipeline_arc.clone();
              async move { p_clone.run(ctx).await.unwrap() }
            },
            criterion::BatchSize::SmallInput,
          );
        },
      );
    }
  }
  group.finish();
}

fn bench_context_data_access(c: &mut Criterion) {
  let mut group = c.benchmark_group("ContextDataAccess");
  let ctx = ContextData::new(BenchContext::default());

  group.bench_function("read_lock", |b| {
    b.iter(|| {
      let _guard = ctx.read();
      criterion::black_box(_guard.counter);
    })
  });

  group.bench_function("write_lock_and_modify", |b| {
    b.iter(|| {
      let mut guard = ctx.write();
      guard.counter += 1;
      criterion::black_box(guard.counter);
    })
  });
  group.finish();
}

fn bench_conditional_dispatch_overhead(c: &mut Criterion) {
  let mut group = c.benchmark_group("ConditionalDispatchOverhead");
  let rt = Runtime::new().unwrap();

  let mut no_op_scoped_pipeline = Pipeline::<ScopedBenchContext, BenchError>::new(["noop_task"]);
  no_op_scoped_pipeline.on_root("noop_task", |_ctx| async { Ok(PipelineControl::Continue) });
  let arc_no_op_scoped = Arc::new(no_op_scoped_pipeline);

  for num_scopes in [1, 5, 10].iter() {
    let mut pipeline = Pipeline::<BenchContext, BenchError>::new(["conditional_step"]);
    let mut builder = pipeline.conditional_scopes_for_step("conditional_step");
    for i in 0..*num_scopes {
      let pipeline_clone = arc_no_op_scoped.clone();
      // Only the first scope matches, so later ones measure pure condition-check overhead.
      let condition_met = i == 0;
      builder = builder
        .add_static_scope(pipeline_clone, |_main_ctx| {
          Ok(ContextData::new(ScopedBenchContext))
        })
        .on_condition(move |_main_ctx| condition_met);
    }
    builder.finalize_conditional_step(false);
    let pipeline_arc = Arc::new(pipeline);

    group.throughput(Throughput::Elements(1));
    group.bench_with_input(BenchmarkId::from_parameter(*num_scopes), num_scopes, |b, _| {
      b.to_async(&rt).iter_batched(
        || ContextData::new(BenchContext::default()),
        |ctx| {
          let p_clone = pipeline_arc.clone();
          async move { p_clone.run(ctx).await.unwrap() }
        },
        criterion::BatchSize::SmallInput,
      );
    });
  }
  group.finish();
}

fn bench_registry_dispatch(c: &mut Criterion) {
  let mut group = c.benchmark_group("RegistryDispatch");
  let rt = Runtime::new().unwrap();

  let orka_registry = Arc::new(Orka::<BenchError>::new());

  let mut p1 = Pipeline::<BenchContext, BenchError>::new(["task1"]);
  p1.on_root("task1", create_sync_increment_handler(1));
  orka_registry.register_pipeline(p1).unwrap();

  // A second pipeline keyed by a different context type, to exercise registry lookup.
  #[derive(Clone, Default)]
  struct AnotherContext {
    val: i32,
  }
  let mut p2 = Pipeline::<AnotherContext, BenchError>::new(["task2"]);
  p2.on_root("task2", |ctx| async move {
    ctx.write().val += 1;
    Ok(PipelineControl::Continue)
  });
  orka_registry.register_pipeline(p2).unwrap();

  group.throughput(Throughput::Elements(1));
  group.bench_function("dispatch_bench_context", |b| {
    b.to_async(&rt).iter_batched(
      || ContextData::new(BenchContext::default()),
      |ctx| {
        let reg_clone = orka_registry.clone();
        async move { reg_clone.run(ctx).await.unwrap() }
      },
      criterion::BatchSize::SmallInput,
    );
  });
  group.bench_function("dispatch_another_context", |b| {
    b.to_async(&rt).iter_batched(
      || ContextData::new(AnotherContext::default()),
      |ctx| {
        let reg_clone = orka_registry.clone();
        async move { reg_clone.run(ctx).await.unwrap() }
      },
      criterion::BatchSize::SmallInput,
    );
  });
  group.finish();
}

criterion_group!(
  benches,
  bench_simple_pipeline_sync_handlers,
  bench_simple_pipeline_async_handlers,
  bench_context_data_access,
  bench_conditional_dispatch_overhead,
  bench_registry_dispatch
);
criterion_main!(benches);
