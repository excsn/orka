//! Out-of-band cancellation: a token a caller holds outside a run, and the future a handler
//! awaits to notice it.
//!
//! [`PipelineControl::Stop`](crate::PipelineControl) is in-band, so only the handler
//! currently executing can end a run. A [`CancelToken`] is how anything else asks for the
//! same wind-down: an operator, a supervising task, a parent run cancelling its fan-out
//! branches.
//!
//! Cancellation is cooperative and never preemptive. Setting a token stops new work from
//! starting; it does not drop an in-flight handler future. That is the same invariant
//! [`FanOutPolicy::FailFast`](crate::FanOutPolicy) already keeps, and it is what lets a
//! cancelled run reach its normal exit so `on_finish` fires and the resource bag releases.
//!
//! Hand-rolled on an `AtomicBool` and a waker vector, because orka depends on no async
//! utility crate and no runtime.

use parking_lot::Mutex;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

struct CancelInner {
  cancelled: AtomicBool,
  wakers: Mutex<Vec<Option<Waker>>>,
}

/// A shared, cheaply cloneable cancellation flag.
///
/// Every [`ContextData`](crate::ContextData) carries one, so [`cancelled`](Self::cancelled)
/// is always available to a handler. A run started through the plain
/// [`run`](crate::Pipeline::run) holds a token nobody ever cancels, whose `cancelled()`
/// future simply never resolves; pass your own through
/// [`run_with_cancel`](crate::Pipeline::run_with_cancel) to make it live.
///
/// ```ignore
/// let token = CancelToken::new();
/// let watcher = token.clone();
/// tokio::spawn(async move { shutdown.recv().await; watcher.cancel(); });
///
/// let (result, outcome) = orka.run_with_cancel_and_outcome(ctx, token).await;
/// ```
pub struct CancelToken(Arc<CancelInner>);

impl CancelToken {
  pub fn new() -> Self {
    CancelToken(Arc::new(CancelInner {
      cancelled: AtomicBool::new(false),
      wakers: Mutex::new(Vec::new()),
    }))
  }

  /// Sets the token and wakes everything awaiting it. Idempotent: a second call does
  /// nothing.
  pub fn cancel(&self) {
    if self.0.cancelled.swap(true, Ordering::SeqCst) {
      return;
    }
    let waiters = std::mem::take(&mut *self.0.wakers.lock());
    for waker in waiters.into_iter().flatten() {
      waker.wake();
    }
  }

  pub fn is_cancelled(&self) -> bool {
    self.0.cancelled.load(Ordering::SeqCst)
  }

  /// Resolves once the token is cancelled, and never otherwise.
  ///
  /// The engine polls the token at step boundaries, which bounds cancellation latency to
  /// one step. A handler that awaits something long (a completion channel under a several
  /// minute budget) closes that gap itself by racing this against its own work:
  ///
  /// ```ignore
  /// tokio::select! {
  ///   _ = ctx.cancellation().cancelled() => Ok(PipelineControl::Stop),
  ///   r = timed("await-completion", budget, rx.recv()) => finish(r),
  /// }
  /// ```
  pub fn cancelled(&self) -> Cancelled {
    Cancelled {
      token: self.clone(),
      slot: None,
    }
  }
}

impl Clone for CancelToken {
  fn clone(&self) -> Self {
    CancelToken(Arc::clone(&self.0))
  }
}

impl Default for CancelToken {
  fn default() -> Self {
    Self::new()
  }
}

impl fmt::Debug for CancelToken {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("CancelToken")
      .field("cancelled", &self.is_cancelled())
      .finish()
  }
}

/// The future returned by [`CancelToken::cancelled`].
///
/// Owns its token, so it is `'static` and can be boxed or spawned.
pub struct Cancelled {
  token: CancelToken,
  slot: Option<usize>,
}

impl Future for Cancelled {
  type Output = ();

  /// The flag is read **under the waker lock**, and that ordering is the safety argument.
  /// `cancel` swaps the flag before it takes the lock, so a `false` read while holding the
  /// lock proves no drain has happened or can happen before this returns, which is what
  /// makes indexing `wakers` sound. Reading the flag before the lock instead would leave a
  /// window in which `cancel` empties the vector and `wakers[i]` is out of bounds.
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
    let this = self.get_mut();
    let mut wakers = this.token.0.wakers.lock();

    if this.token.is_cancelled() {
      return Poll::Ready(());
    }

    match this.slot {
      Some(i) => {
        if !wakers[i].as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
          wakers[i] = Some(cx.waker().clone());
        }
      }
      None => {
        let i = match wakers.iter().position(|w| w.is_none()) {
          Some(free) => free,
          None => {
            wakers.push(None);
            wakers.len() - 1
          }
        };
        wakers[i] = Some(cx.waker().clone());
        this.slot = Some(i);
      }
    }

    Poll::Pending
  }
}

impl Drop for Cancelled {
  /// Uses `get_mut` rather than indexing because this is the one place that can run after
  /// [`CancelToken::cancel`] has emptied the vector, and a panic here would be a panic in a
  /// destructor during cancellation.
  fn drop(&mut self) {
    let Some(i) = self.slot else { return };
    if let Some(entry) = self.token.0.wakers.lock().get_mut(i) {
      *entry = None;
    }
  }
}

impl fmt::Debug for Cancelled {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Cancelled").field("token", &self.token).finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::task::{RawWaker, RawWakerVTable, Waker};

  fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
      RawWaker::new(std::ptr::null(), &VTABLE)
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
  }

  fn poll_once(fut: &mut Cancelled) -> Poll<()> {
    let waker = noop_waker();
    Pin::new(fut).poll(&mut Context::from_waker(&waker))
  }

  /// Repolling the same waiter must reuse its slot rather than appending, or a handler that
  /// races the token inside a loop grows the vector for the life of the run.
  #[test]
  fn repolling_one_waiter_registers_a_single_slot() {
    let token = CancelToken::new();
    let mut waiter = token.cancelled();

    for _ in 0..100 {
      assert!(poll_once(&mut waiter).is_pending());
    }

    assert_eq!(token.0.wakers.lock().len(), 1);
  }

  /// A dropped `select!` arm frees its slot, and the next waiter takes it back rather than
  /// extending the vector.
  #[test]
  fn a_dropped_waiter_frees_its_slot_for_reuse() {
    let token = CancelToken::new();

    for _ in 0..100 {
      let mut waiter = token.cancelled();
      assert!(poll_once(&mut waiter).is_pending());
      assert_eq!(token.0.wakers.lock().len(), 1);
    }

    assert_eq!(token.0.wakers.lock().iter().filter(|w| w.is_some()).count(), 0);
  }

  #[test]
  fn concurrent_waiters_each_get_their_own_slot() {
    let token = CancelToken::new();
    let mut a = token.cancelled();
    let mut b = token.cancelled();

    assert!(poll_once(&mut a).is_pending());
    assert!(poll_once(&mut b).is_pending());

    assert_eq!(token.0.wakers.lock().len(), 2);
  }

  #[test]
  fn a_cancelled_token_resolves_without_registering() {
    let token = CancelToken::new();
    token.cancel();
    let mut waiter = token.cancelled();

    assert!(poll_once(&mut waiter).is_ready());
    assert!(token.0.wakers.lock().is_empty());
  }
}
