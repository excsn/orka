//! Run-scoped RAII resources: values a run must *hold*, as opposed to data a run
//! *operates on*.

use parking_lot::Mutex;
use std::any::Any;

/// A bag of run-scoped resources, reachable from any handler via
/// [`ContextData::resources`](crate::ContextData::resources).
///
/// Some things a run acquires are not part of its data model at all: a mutex guard held
/// for the duration of a build, a `TempDir` that must outlive the steps writing into it,
/// an open file handle. Threading those through the context struct as `Option<T>` fields
/// works, but it makes the context type claim they are workflow data, and it pushes drop
/// ordering into a hand-written finish handler. Put them here instead:
///
/// ```ignore
/// // in the step that acquires them
/// ctx.resources().put(lock_guard).put(temp_dir);
/// ctx.with_mut(|c| c.build_dir = path);   // the *path* is data; the handle is not
/// ```
///
/// Everything stashed is dropped when the run finishes, in **reverse** order of insertion
/// (the RAII stack discipline), after any [`on_finish`](crate::Pipeline::on_finish)
/// handlers have run. That ordering is deliberate: a finalizer can still copy artifacts
/// out of a temp dir before the dir is removed, or write a last record before the lock is
/// released.
///
/// ## Scope and limits
///
/// - Only a full [`run`](crate::Pipeline::run) releases the bag. The partial runners
///   ([`run_step`](crate::Pipeline::run_step) and friends) leave it alone, exactly as they
///   leave the finish ring alone, so a step-isolation test can stash something and still
///   inspect it afterwards. Nothing leaks either way: whatever is still held drops when
///   the last [`ContextData`](crate::ContextData) handle for that context drops.
/// - Rust has no async `Drop`, so this is for resources whose cleanup is synchronous and
///   quick. Anything that needs to `await` (committing a transaction, draining a
///   connection) belongs in an `on_finish` handler, which is awaited.
/// - [`ContextData::project`](crate::ContextData::project) builds an independent context,
///   so the projection starts with an empty bag of its own.
pub struct RunResources {
  entries: Mutex<Vec<Box<dyn Any + Send>>>,
}

impl RunResources {
  pub(crate) fn new() -> Self {
    Self {
      entries: Mutex::new(Vec::new()),
    }
  }

  /// Stashes a resource, to be dropped when the run finishes. Returns `&Self`, so several
  /// can be chained: `ctx.resources().put(guard).put(temp_dir)`.
  pub fn put<R: Send + 'static>(&self, resource: R) -> &Self {
    self.entries.lock().push(Box::new(resource));
    self
  }

  /// Borrows the most recently stashed resource of type `R` and runs `f` on it, returning
  /// `None` if nothing of that type is held.
  ///
  /// This is how a resource that also carries a usable value stays reachable, without
  /// duplicating it into the context:
  ///
  /// ```ignore
  /// let path = ctx.resources().with(|t: &TempDir| t.path().to_path_buf());
  /// ```
  ///
  /// The bag's lock is held while `f` runs, so `f` must not touch the same bag again
  /// (`put`/`with` from inside `f` deadlocks) and must not block.
  pub fn with<R, F, T>(&self, f: F) -> Option<T>
  where
    R: Send + 'static,
    F: FnOnce(&R) -> T,
  {
    let entries = self.entries.lock();
    entries.iter().rev().find_map(|entry| entry.downcast_ref::<R>()).map(f)
  }

  /// Removes the most recently stashed resource of type `R` and hands over ownership.
  ///
  /// Use this only when the resource genuinely is not coming back. If you mean to operate
  /// on it and leave it under the bag's care, use [`take_guard`](Self::take_guard):
  /// between a `take` and a manual `put` the value lives in a local, so an early `?`, a
  /// timeout, or a panic drops it there instead of at the run's release point, which is
  /// exactly the smuggling this bag exists to end.
  pub fn take<R: Send + 'static>(&self) -> Option<R> {
    let mut entries = self.entries.lock();
    let position = entries.iter().rposition(|entry| entry.is::<R>())?;
    let taken = entries.remove(position);
    taken.downcast::<R>().ok().map(|boxed| *boxed)
  }

  /// Takes the most recently stashed resource of type `R` out on loan, returning it to the
  /// bag when the guard drops.
  ///
  /// [`with`](Self::with) holds the bag's lock for the duration of its closure, so a
  /// resource borrowed that way cannot be used across an `.await`. That suits values a run
  /// merely holds, but not ones it *operates* on: a stream sender that chunks are awaited
  /// into needs `&mut` across suspension points. This hands out ownership for exactly that,
  /// without giving up the bag's guarantee.
  ///
  /// ```ignore
  /// let mut sender = ctx.resources().take_guard::<StreamSender>().expect("stashed at open");
  /// for chunk in chunks {
  ///   sender.send(chunk).await?;   // the guard may cross awaits; it is not a lock guard
  /// }
  /// // dropping the guard puts the sender back
  /// ```
  ///
  /// Because the return happens in `Drop`, no path skips it: an early `?`, a panic, or a
  /// handler cancelled by a timeout all put the resource back, so it is still released at
  /// the run's defined point after [`on_finish`](crate::Pipeline::on_finish) rather than
  /// mid-await inside a dropped future.
  pub fn take_guard<R: Send + 'static>(&self) -> Option<TakenResource<'_, R>> {
    self.take::<R>().map(|value| TakenResource {
      value: Some(value),
      bag: self,
    })
  }

  /// How many resources are currently held.
  pub fn len(&self) -> usize {
    self.entries.lock().len()
  }

  /// Whether nothing is currently held.
  pub fn is_empty(&self) -> bool {
    self.entries.lock().is_empty()
  }

  /// Drops everything held, most recently stashed first. Returns how many were released.
  ///
  /// The entries are moved out from under the lock before being dropped, so a resource
  /// whose `Drop` reaches back into the same context cannot deadlock.
  pub(crate) fn release_all(&self) -> usize {
    let mut taken = std::mem::take(&mut *self.entries.lock());
    let count = taken.len();
    // Explicit reverse drain: `Vec`'s own `Drop` would release front to back, which is
    // the wrong order for RAII.
    while let Some(resource) = taken.pop() {
      drop(resource);
    }
    count
  }
}

impl std::fmt::Debug for RunResources {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("RunResources").field("held", &self.len()).finish()
  }
}

/// A resource taken out of a [`RunResources`] bag on loan, returned when dropped.
///
/// Obtained from [`RunResources::take_guard`]. Deref gives access to the value; the
/// guard itself is an ordinary owned value rather than a lock guard, so holding it across
/// an `.await` is fine and is the whole point.
pub struct TakenResource<'a, R: Send + 'static> {
  value: Option<R>,
  bag: &'a RunResources,
}

impl<R: Send + 'static> TakenResource<'_, R> {
  /// Keeps the resource instead of returning it to the bag, consuming the guard.
  ///
  /// From here it is an ordinary value and the bag no longer releases it for you.
  pub fn keep(mut self) -> R {
    self.value.take().expect("a live guard always holds its value")
  }
}

impl<R: Send + 'static> std::ops::Deref for TakenResource<'_, R> {
  type Target = R;

  fn deref(&self) -> &R {
    self.value.as_ref().expect("a live guard always holds its value")
  }
}

impl<R: Send + 'static> std::ops::DerefMut for TakenResource<'_, R> {
  fn deref_mut(&mut self) -> &mut R {
    self.value.as_mut().expect("a live guard always holds its value")
  }
}

impl<R: Send + 'static> Drop for TakenResource<'_, R> {
  fn drop(&mut self) {
    if let Some(value) = self.value.take() {
      self.bag.put(value);
    }
  }
}

impl<R: Send + 'static + std::fmt::Debug> std::fmt::Debug for TakenResource<'_, R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_tuple("TakenResource").field(&self.value).finish()
  }
}
