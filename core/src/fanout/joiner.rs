//! A bounded-concurrency joiner over boxed futures, written by hand because orka depends
//! on no async utility crate and cannot spawn.

use crate::core::cancel::CancelToken;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A branch future. Boxing makes it `Unpin`, which is what lets [`BoundedJoin`] poll a
/// collection of them without pin-projection or unsafe.
pub(crate) type BranchFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Decides whether a finished branch should stop the remaining ones from starting. This is
/// how a fail-fast policy reaches the joiner without the joiner knowing about policies.
pub(crate) type StopPredicate<T> = Box<dyn Fn(&T) -> bool + Send>;

/// Drives up to `limit` futures at a time to completion, resolving to their outputs in
/// **input order** regardless of the order they finish in.
///
/// This is cooperative concurrency, not parallelism: every branch is polled on the caller's
/// task, so branches make progress while each other awaits, but a branch that blocks the
/// thread (a synchronous file read, a long CPU stretch, a lock guard held across a yield)
/// stalls its siblings.
///
/// A branch that never starts is never polled, and since async blocks are lazy it has run
/// no code at all. Its slot resolves to `None`.
pub(crate) struct BoundedJoin<T> {
  /// Slot `i` holds branch `i` until it completes, then `None`.
  futures: Vec<Option<BranchFuture<T>>>,
  /// Slot `i` holds branch `i`'s output once it finishes; `None` means it never started.
  results: Vec<Option<T>>,
  /// Indices currently in flight. Length never exceeds `limit`.
  active: Vec<usize>,
  /// The next index that has not been started yet.
  next: usize,
  limit: usize,
  /// Set once `stop_on` trips: no further branches start, but in-flight ones drain.
  stop_starting: bool,
  stop_on: Option<StopPredicate<T>>,
  /// Governs starting exactly as `stop_starting` does, but is set from outside rather than
  /// by a finished branch.
  cancel: Option<CancelToken>,
}

impl<T> BoundedJoin<T> {
  /// # Panics
  /// Panics if `limit` is zero, which would be a setup error that could never make
  /// progress.
  pub(crate) fn new(
    futures: Vec<BranchFuture<T>>,
    limit: usize,
    stop_on: Option<StopPredicate<T>>,
    cancel: Option<CancelToken>,
  ) -> Self {
    assert!(limit > 0, "Orka setup error: fan-out concurrency limit must be at least 1.");
    let count = futures.len();
    let mut results = Vec::with_capacity(count);
    results.resize_with(count, || None);
    Self {
      futures: futures.into_iter().map(Some).collect(),
      results,
      active: Vec::new(),
      next: 0,
      limit,
      stop_starting: false,
      stop_on,
      cancel,
    }
  }

  fn cancelled(&self) -> bool {
    self.cancel.as_ref().is_some_and(|c| c.is_cancelled())
  }
}

// Safe and deliberate: this future needs no address stability of its own. Every branch is
// already a `Pin<Box<_>>` whose pinning guarantee comes from the heap allocation, and
// moving the struct moves only the `Vec` headers, never a future.
impl<T> Unpin for BoundedJoin<T> {}

impl<T> Future for BoundedJoin<T> {
  /// Index-aligned with the input. `None` means that branch never started.
  type Output = Vec<Option<T>>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    // Every field is `Unpin`, so the three `Vec`s below are ordinary disjoint field
    // borrows rather than pin projections.
    let this = self.get_mut();

    // Fill-then-poll, repeated while the round makes progress. Looping is load-bearing,
    // not an optimisation: a future that has never been polled has never registered a
    // waker, so filling a freed slot and returning `Pending` without polling the newcomer
    // would park the whole join forever.
    loop {
      let halted = this.stop_starting || this.cancelled();

      while !halted && this.active.len() < this.limit && this.next < this.futures.len() {
        this.active.push(this.next);
        this.next += 1;
      }

      let mut completed_any = false;
      let mut i = 0;
      while i < this.active.len() {
        let index = this.active[i];
        let branch = this.futures[index]
          .as_mut()
          .expect("an active slot always holds its future");

        match branch.as_mut().poll(cx) {
          Poll::Ready(output) => {
            if let Some(stop_on) = this.stop_on.as_ref()
              && stop_on(&output)
            {
              this.stop_starting = true;
            }
            this.results[index] = Some(output);
            this.futures[index] = None;
            this.active.swap_remove(i); // do not advance `i`: a new index now sits here
            completed_any = true;
          }
          Poll::Pending => i += 1,
        }
      }

      // Recomputed rather than reusing `halted`: a branch finishing in the round above can
      // trip `stop_on`, and another task can cancel while this round runs.
      let halted = this.stop_starting || this.cancelled();

      if this.active.is_empty() && (halted || this.next >= this.futures.len()) {
        return Poll::Ready(std::mem::take(&mut this.results));
      }

      // Go round again only when a completion actually freed a slot there is work to
      // fill, which is the case where a newcomer still needs its first poll. Otherwise
      // every live branch has registered its waker and there is nothing more to do.
      let can_start_more = !halted && this.next < this.futures.len() && this.active.len() < this.limit;
      if !(completed_any && can_start_more) {
        return Poll::Pending;
      }
    }
  }
}
