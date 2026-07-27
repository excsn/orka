//! Opting a fan-out into real parallelism by delegating task spawning to the consumer.

use std::future::Future;
use std::pin::Pin;

/// Work handed to a [`TaskSpawner`] to run to completion.
pub type SpawnedTask = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A handle that resolves once a spawned task has finished, however it finished. A task
/// that panicked or was aborted must still resolve its handle rather than hanging.
pub type SpawnHandle = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Runs fan-out branches on a real executor instead of on the caller's task.
///
/// orka depends on no async runtime and never spawns, so by default a
/// [`FanOut`](crate::FanOut) polls its branches cooperatively: they interleave at await
/// points but share one task, and a branch that blocks the thread stalls its siblings.
/// Supplying a spawner lifts that: each branch becomes a task on your runtime, so branches
/// run on different threads and CPU-bound work no longer serialises.
///
/// For tokio, enable orka's `tokio` feature and use the shipped [`TokioSpawner`]:
///
/// ```ignore
/// let results = FanOut::new(pipeline)
///   .spawner(Arc::new(TokioSpawner))
///   .max_concurrent(8)
///   .run(items)
///   .await;
/// ```
///
/// For anything else the implementation is short enough to paste:
///
/// ```ignore
/// struct MySpawner;
///
/// impl TaskSpawner for MySpawner {
///   fn spawn(&self, task: SpawnedTask) -> SpawnHandle {
///     let handle = my_runtime::spawn(task);
///     // Resolve on panic or abort too; orka reports that branch as lost rather than hanging.
///     Box::pin(async move { let _ = handle.await; })
///   }
/// }
/// ```
///
/// Two behaviours change when a spawner is in play, both worth knowing:
///
/// - **A panicking branch is contained.** Cooperatively, a branch that panics unwinds the
///   whole fan-out and the caller with it. Spawned, the runtime catches it, the handle
///   resolves without a result, and orka reports that branch as failed with
///   [`OrkaError::FanOutBranchLost`](crate::OrkaError::FanOutBranchLost). The other
///   branches are unaffected.
/// - **Dropping the fan-out no longer cancels in-flight work.** Cooperative branches are
///   owned by the fan-out future and stop when it is dropped; spawned ones belong to the
///   runtime and keep running detached.
///
/// Branch *starting* is still governed by [`max_concurrent`](crate::FanOut::max_concurrent)
/// and by fail-fast, because a branch only spawns when the fan-out first polls it.
pub trait TaskSpawner: Send + Sync + 'static {
  /// Starts `task` and returns a handle resolving when it has finished.
  fn spawn(&self, task: SpawnedTask) -> SpawnHandle;
}

/// A [`TaskSpawner`] backed by `tokio::spawn`. Requires orka's `tokio` feature.
///
/// The feature pulls in tokio with only its `rt` feature, which is all `tokio::spawn`
/// needs. orka itself still depends on no runtime; this is opt-in.
///
/// Like any use of `tokio::spawn`, this must be called from within a tokio runtime, which
/// a fan-out being awaited inside one always is.
#[cfg(feature = "tokio")]
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

#[cfg(feature = "tokio")]
impl TaskSpawner for TokioSpawner {
  fn spawn(&self, task: SpawnedTask) -> SpawnHandle {
    let handle = tokio::spawn(task);
    // Deliberately discard the JoinError: a panicked or aborted task must still resolve
    // its handle, so the fan-out reports that branch as lost rather than hanging on it.
    Box::pin(async move {
      let _ = handle.await;
    })
  }
}
