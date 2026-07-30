use crate::core::cancel::CancelToken;
use crate::core::resources::RunResources;
use crate::error::OrkaError;
use parking_lot::{
  MappedRwLockReadGuard,
  MappedRwLockWriteGuard,
  RwLock,
  RwLockReadGuard,
  RwLockWriteGuard,
};
use std::fmt;
use std::sync::Arc;

/// The shared state behind a `ContextData`: the workflow's data, plus the run-scoped
/// resources it is holding and the token that can wind the run down.
struct Inner<T: Send + Sync + 'static> {
  data: RwLock<T>,
  resources: RunResources,
  cancel: RwLock<CancelToken>,
}

/// A wrapper for context data providing shared ownership and interior mutability
/// using parking_lot::RwLock.
///
/// IMPORTANT: Lock guards obtained from this struct are blocking and MUST NOT
/// be held across `.await` suspension points in asynchronous code. The scoped accessors
/// [`with_ref`](Self::with_ref) and [`with_mut`](Self::with_mut) enforce that structurally.
///
/// Alongside the data, each context carries a [`RunResources`] bag for RAII values the run
/// must hold but does not operate on (lock guards, temp dirs); see
/// [`resources`](Self::resources). It also carries the run's
/// [`CancelToken`](Self::cancellation).
pub struct ContextData<T: Send + Sync + 'static>(Arc<Inner<T>>);

impl<T: Send + Sync + 'static> ContextData<T> {
  pub fn new(data: T) -> Self {
    ContextData(Arc::new(Inner {
      data: RwLock::new(data),
      resources: RunResources::new(),
      cancel: RwLock::new(CancelToken::new()),
    }))
  }

  /// The run-scoped resource bag shared by every handle to this context.
  ///
  /// Stash a value here when the run needs to *hold* it rather than *use* it, so the
  /// context type does not have to carry it as data:
  ///
  /// ```ignore
  /// ctx.resources().put(lock_guard).put(temp_dir);
  /// ```
  ///
  /// Everything stashed is dropped in reverse order at the end of a full
  /// [`run`](crate::Pipeline::run), after its `on_finish` handlers. See [`RunResources`]
  /// for the full lifecycle, including what the partial runners do.
  pub fn resources(&self) -> &RunResources {
    &self.0.resources
  }

  /// The run's cancellation token, shared by every handle to this context.
  ///
  /// A context always has one, so this never returns `None` and a handler never has to
  /// branch on whether the run is cancellable. A context that was never passed to
  /// [`run_with_cancel`](crate::Pipeline::run_with_cancel) holds a token nobody can reach,
  /// so [`is_cancelled`](CancelToken::is_cancelled) stays false and
  /// [`cancelled`](CancelToken::cancelled) never resolves:
  ///
  /// ```ignore
  /// tokio::select! {
  ///   _ = ctx.cancellation().cancelled() => Ok(PipelineControl::Stop),
  ///   r = timed("await-completion", budget, rx.recv()) => finish(r),
  /// }
  /// ```
  ///
  /// The engine also polls it at every step boundary, so a handler that cannot await it
  /// still gets cancellation with a latency of one step. Calling
  /// [`cancel`](CancelToken::cancel) on it from inside a handler cancels this run.
  pub fn cancellation(&self) -> CancelToken {
    self.0.cancel.read().clone()
  }

  /// Replaces the run's token. Called once at the start of a cancellable run, and by
  /// fan-out and conditional scopes to hand a child run its parent's token.
  pub(crate) fn install_cancellation(&self, token: CancelToken) {
    *self.0.cancel.write() = token;
  }

  /// Acquires a read lock. Panics if the RwLock is poisoned.
  /// The returned guard MUST be dropped before any `.await` point.
  pub fn read(&self) -> RwLockReadGuard<'_, T> {
    self.0.data.read()
  }

  /// Acquires a write lock. Panics if the RwLock is poisoned.
  /// The returned guard MUST be dropped before any `.await` point.
  pub fn write(&self) -> RwLockWriteGuard<'_, T> {
    self.0.data.write()
  }

  /// Attempts to acquire a read lock without blocking.
  pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
    self.0.data.try_read()
  }

  /// Attempts to acquire a write lock without blocking.
  pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
    self.0.data.try_write()
  }

  // Helper for extracting a part of the context under a read lock
  // Useful if T is a struct and you want a guard to just one field.
  // Example: context_data.map_read(|data| &data.some_field)
  pub fn map_read<F, U: ?Sized>(&self, f: F) -> MappedRwLockReadGuard<'_, U>
  where
    F: FnOnce(&T) -> &U,
  {
    RwLockReadGuard::map(self.read(), f)
  }

  // Helper for extracting a part of the context under a write lock
  pub fn map_write<F, U: ?Sized>(&self, f: F) -> MappedRwLockWriteGuard<'_, U>
  where
    F: FnOnce(&mut T) -> &mut U,
  {
    RwLockWriteGuard::map(self.write(), f)
  }

  /// Runs `f` under a read lock, releasing the guard before returning its result.
  ///
  /// This is the await-safe way to read. `ContextData` asks callers to follow exactly one
  /// rule (never hold a guard across an `.await`), and this makes that rule structural
  /// rather than a convention: `f` is synchronous and the guard's scope is this call, so
  /// there is no way to carry the lock into a suspension point.
  ///
  /// ```ignore
  /// let url = ctx.with_ref(|c| c.url.clone());   // guard already released
  /// let body = http::get(&url).await?;           // safe by construction
  /// ```
  pub fn with_ref<F, R>(&self, f: F) -> R
  where
    F: FnOnce(&T) -> R,
  {
    let guard = self.read();
    f(&guard)
  }

  /// Reads a value a previous step was supposed to have produced, failing with
  /// [`OrkaError::ResourceMissing`] rather than panicking when it is absent.
  ///
  /// This is the runtime counterpart to declaring the dependency with
  /// [`produces`](crate::Pipeline::produces) / [`consumed_by`](crate::Pipeline::consumed_by):
  ///
  /// ```ignore
  /// let spec = ctx.require(Res::AppSpec, |c| c.app_spec.clone())?;
  /// ```
  ///
  /// It replaces the `.expect("app_spec set by load-spec step")` that would otherwise sit
  /// at every consuming site. The gain is not tidiness but cleanup: a panic unwinds past
  /// the run's [`on_finish`](crate::Pipeline::on_finish) ring and past
  /// [`RunResources`] release, and whatever does drop, drops in the
  /// wrong order, since the bag's own `Drop` releases front to back rather than in reverse.
  /// A handled error leaves all of that intact.
  ///
  /// Note this does **not** verify the resource against the `produces`/`consumed_by`
  /// declarations: nothing checks that the name you require is one you declared, because a
  /// context does not know which step is reading it. What ties the two together is using
  /// the same key in both places, so a typed `Res` enum makes a rename move both at once.
  pub fn require<R, F>(&self, resource: impl AsRef<str>, get: F) -> Result<R, OrkaError>
  where
    F: FnOnce(&T) -> Option<R>,
  {
    self.with_ref(get).ok_or_else(|| OrkaError::ResourceMissing {
      resource: resource.as_ref().to_string(),
    })
  }

  /// Runs `f` under a write lock, releasing the guard before returning its result.
  ///
  /// The mutating counterpart of [`with_ref`](Self::with_ref), with the same structural
  /// guarantee: a scoped `&mut T` that cannot escape into an `.await`.
  ///
  /// ```ignore
  /// ctx.with_mut(|c| c.specs.retain(|s| s.enabled));
  /// let previous = ctx.with_mut(|c| std::mem::take(&mut c.pending));  // returns a value
  /// ```
  ///
  /// Note this hands you `&mut T`, so it reaches any field by value. A field that is
  /// itself an `Arc<U>` shared outside the context is a different problem: mutating `U`
  /// in place through it is [`Arc::get_mut`]'s job, not this one. If the field is only
  /// `Arc`'d to hand a non-`Clone` value to readers, [`map_read`](Self::map_read) already
  /// borrows a field without cloning, and the `Arc` may not be needed at all.
  pub fn with_mut<F, R>(&self, f: F) -> R
  where
    F: FnOnce(&mut T) -> R,
  {
    let mut guard = self.write();
    f(&mut guard)
  }

  /// Builds a *new, independent* `ContextData<U>` from a projection of this one.
  ///
  /// This is the common shape of a sub-context extractor: take a read lock, pull out
  /// (usually clone) the part a scoped pipeline cares about, and hand it over as its
  /// own context.
  ///
  /// ```ignore
  /// pipeline.set_extractor("validate", |main| Ok(main.project(|d| d.customer.clone())));
  /// ```
  ///
  /// The result does **not** share state with `self`: writes to it are not visible here.
  /// To propagate them back, pair the extractor with a merge function via
  /// [`Pipeline::set_extractor_with_merge`](crate::Pipeline::set_extractor_with_merge).
  /// It also starts with an empty [`resources`](Self::resources) bag of its own.
  ///
  /// The read guard is released before this returns, so the result is safe to hold
  /// across an `.await`.
  pub fn project<U, F>(&self, get: F) -> ContextData<U>
  where
    U: Send + Sync + 'static,
    F: FnOnce(&T) -> U,
  {
    let projected = {
      let guard = self.read();
      get(&*guard)
    };
    ContextData::new(projected)
  }
}

impl<T: Send + Sync + 'static> Clone for ContextData<T> {
  fn clone(&self) -> Self {
    ContextData(Arc::clone(&self.0))
  }
}

impl<T: Send + Sync + 'static + fmt::Debug> fmt::Debug for ContextData<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ContextData")
      .field("data", &self.0.data)
      .field("resources", &self.0.resources)
      .field("cancel", &*self.0.cancel.read())
      .finish()
  }
}

impl<T: Send + Sync + 'static + Default> Default for ContextData<T> {
  fn default() -> Self {
    Self::new(Default::default())
  }
}
