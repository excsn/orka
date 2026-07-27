//! Defines the `Handler<TData>` type for pipeline step handlers, operating on `ContextData<TData>`.
//! Also includes mechanisms for sub-context extraction.

use crate::core::context_data::ContextData;
use crate::core::control::PipelineControl;
use crate::error::{OrkaError, OrkaResult};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;


/// Type alias for a pipeline step handler.
///
/// A handler is an asynchronous function that takes ownership of a `ContextData<TData>`
/// instance (typically a clone of the main context data `Arc`) and returns a `Future`
/// resolving to `OrkaResult<PipelineControl>`.
///
/// `TData` is the underlying data type stored within `ContextData<TData>`.
///
/// Handlers are responsible for:
/// 1. Acquiring locks (`.read()` or `.write()`) on the `ContextData` to access or modify state.
/// 2. **Crucially, ensuring that lock guards are dropped BEFORE any `.await` suspension point.**
/// 3. Performing their logic, possibly including I/O operations.
/// 4. Returning `PipelineControl::Continue` to proceed or `PipelineControl::Stop` to halt the pipeline.
pub type Handler<TData, Err> = Box<
  dyn Fn(ContextData<TData>) -> Pin<Box<dyn Future<Output = Result<PipelineControl, Err>> + Send>>
    + Send
    + Sync,
>;

/// Type alias for a run-level finish handler registered via
/// [`Pipeline::on_finish`](crate::Pipeline::on_finish).
///
/// Invoked, awaited, on every exit of a full `run()` with the final shared context and the
/// run's [`RunOutcome`](crate::RunOutcome).
pub type FinishHandler<TData, Err> = Box<
  dyn Fn(ContextData<TData>, crate::core::trace::RunOutcome) -> Pin<Box<dyn Future<Output = Result<(), Err>> + Send>>
    + Send
    + Sync,
>;


/// Trait for a type-erased extractor that can get a sub-context `ContextData<SData>`
/// from a root context `ContextData<TData>`.
///
/// The extraction itself might be fallible.
pub trait AnyContextDataExtractor<TData: 'static + Send + Sync>: Send + Sync {
  /// Extracts a `ContextData<SData>` for a sub-context.
  /// The actual `SData` type is erased at this trait level.
  /// The returned `Box<dyn Any + Send>` should effectively contain `ContextData<SData>`.
  fn extract_sub_context_data(&self, root_ctx_data: ContextData<TData>) -> OrkaResult<Box<dyn Any + Send>>;

  /// Returns the TypeId of the sub-context's underlying data type `SData` this extractor targets.
  fn sub_context_data_type_id(&self) -> std::any::TypeId;

  /// Folds the (possibly mutated) sub-context back into the root context.
  ///
  /// `sub_ctx_data` must be the `ContextData<SData>` this extractor produced. Extractors
  /// registered without a merge function treat this as a no-op, preserving the historical
  /// "the sub-handler works on a detached copy" semantics.
  ///
  /// Callers must not hold any lock guard when invoking this — it takes a read lock on the
  /// sub-context and a write lock on the root.
  fn merge_sub_context_data(
    &self,
    _root_ctx_data: ContextData<TData>,
    _sub_ctx_data: &(dyn Any + Send),
  ) -> OrkaResult<()> {
    Ok(())
  }

  /// Whether this extractor was registered with a merge function.
  fn has_merge(&self) -> bool {
    false
  }
}

/// A function folding a mutated sub-context `SData` back into the root `TData`.
pub type MergeFn<TData, SData> = Arc<dyn Fn(&mut TData, &SData) + Send + Sync + 'static>;

/// A function deriving a sub-context `ContextData<SData>` from a root `ContextData<TData>`.
///
/// Extraction is fallible at the framework level, hence the `OrkaError`.
pub type ExtractorFn<TData, SData> =
  Arc<dyn Fn(ContextData<TData>) -> Result<ContextData<SData>, OrkaError> + Send + Sync + 'static>;

/// A predicate over a context, used to decide whether a conditional scope should run.
pub type ConditionFn<TData> = Arc<dyn Fn(ContextData<TData>) -> bool + Send + Sync + 'static>;

pub struct ContextDataExtractorImpl<
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync, // SData is the underlying data type for the sub-context
> {
  extractor_fn: Arc<dyn Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>> + Send + Sync + 'static>,
  // Optional write-back. `None` means the sub-context is detached (legacy behaviour).
  merge_fn: Option<MergeFn<TData, SData>>,
}

impl<TData: 'static + Send + Sync, SData: 'static + Send + Sync> ContextDataExtractorImpl<TData, SData> {
  pub fn new(f: impl Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>> + Send + Sync + 'static) -> Self {
    Self {
      extractor_fn: Arc::new(f),
      merge_fn: None,
    }
  }

  /// Builds an extractor that also folds the sub-context back into the root on success.
  pub fn with_merge(
    f: impl Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>> + Send + Sync + 'static,
    merge: impl Fn(&mut TData, &SData) + Send + Sync + 'static,
  ) -> Self {
    Self {
      extractor_fn: Arc::new(f),
      merge_fn: Some(Arc::new(merge)),
    }
  }
}

impl<TData: 'static + Send + Sync, SData: 'static + Send + Sync> AnyContextDataExtractor<TData>
  for ContextDataExtractorImpl<TData, SData>
{
  fn extract_sub_context_data(&self, root_ctx_data: ContextData<TData>) -> OrkaResult<Box<dyn Any + Send>> {
    let sub_ctx_data: ContextData<SData> = (self.extractor_fn)(root_ctx_data)?;
    Ok(Box::new(sub_ctx_data))
  }

  fn sub_context_data_type_id(&self) -> std::any::TypeId {
    std::any::TypeId::of::<SData>()
  }

  fn merge_sub_context_data(
    &self,
    root_ctx_data: ContextData<TData>,
    sub_ctx_data: &(dyn Any + Send),
  ) -> OrkaResult<()> {
    let Some(merge) = self.merge_fn.as_ref() else {
      return Ok(());
    };

    let sub: &ContextData<SData> = sub_ctx_data.downcast_ref::<ContextData<SData>>().ok_or_else(|| {
      OrkaError::Internal(format!(
        "Internal type mismatch merging sub-context back: expected ContextData<{}>.",
        std::any::type_name::<SData>()
      ))
    })?;

    // Take the sub read guard first, then the root write guard, and drop both here.
    // No `.await` may occur inside this scope.
    let sub_guard = sub.read();
    let mut root_guard = root_ctx_data.write();
    merge(&mut *root_guard, &*sub_guard);

    Ok(())
  }

  fn has_merge(&self) -> bool {
    self.merge_fn.is_some()
  }
}

pub(crate) fn downcast_context_data<SData: 'static + Send + Sync>(
  any_ctx_data: Box<dyn Any + Send>,
  expected_sdata_type_id: std::any::TypeId,
  step_name: &str,
) -> OrkaResult<ContextData<SData>> {
  if std::any::TypeId::of::<SData>() != expected_sdata_type_id {
    return Err(OrkaError::TypeMismatch {
      step_name: step_name.to_string(),
      expected_type: format!(
        "ContextData<{}> (underlying SData TypeId: {:?})",
        std::any::type_name::<SData>(),
        std::any::TypeId::of::<SData>()
      ),
    });
  }

  match any_ctx_data.downcast::<ContextData<SData>>() {
    Ok(boxed_ctx_data) => Ok(*boxed_ctx_data),
    Err(_) => {
      Err(OrkaError::Internal(format!(
              "Internal type mismatch during ContextData downcast for step '{}'. Expected ContextData<{}> but downcast failed despite TypeId match.",
              step_name,
              std::any::type_name::<SData>()
          )))
    }
  }
}
