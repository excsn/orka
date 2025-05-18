// core/src/core/context_data.rs (or wherever you place it)
use parking_lot::{
  MappedRwLockReadGuard,
  MappedRwLockWriteGuard, // Useful for "extracting" parts
  RwLock,
  RwLockReadGuard,
  RwLockWriteGuard,
};
use std::sync::Arc;

/// A wrapper for context data providing shared ownership and interior mutability
/// using parking_lot::RwLock.
///
/// IMPORTANT: Lock guards obtained from this struct are blocking and MUST NOT
/// be held across `.await` suspension points in asynchronous code.
#[derive(Debug)]
pub struct ContextData<T: Send + Sync + 'static>(Arc<RwLock<T>>);

impl<T: Send + Sync + 'static> ContextData<T> {
  pub fn new(data: T) -> Self {
    ContextData(Arc::new(RwLock::new(data)))
  }

  /// Acquires a read lock. Panics if the RwLock is poisoned.
  /// The returned guard MUST be dropped before any `.await` point.
  pub fn read(&self) -> RwLockReadGuard<'_, T> {
    self.0.read() // parking_lot's read doesn't return Result on success
  }

  /// Acquires a write lock. Panics if the RwLock is poisoned.
  /// The returned guard MUST be dropped before any `.await` point.
  pub fn write(&self) -> RwLockWriteGuard<'_, T> {
    self.0.write()
  }

  /// Attempts to acquire a read lock without blocking.
  pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
    self.0.try_read()
  }

  /// Attempts to acquire a write lock without blocking.
  pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
    self.0.try_write()
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
}

impl<T: Send + Sync + 'static> Clone for ContextData<T> {
  fn clone(&self) -> Self {
    ContextData(Arc::clone(&self.0))
  }
}

impl<T: Send + Sync + 'static + Default> Default for ContextData<T> {
  fn default() -> Self {
    Self::new(Default::default())
  }
}
