// orka/src/core/context.rs

//! Defines the `Handler<TData>` type for pipeline step handlers, operating on `ContextData<TData>`.
//! Also includes (or will include) mechanisms for sub-context extraction.

use crate::core::context_data::ContextData; // Import the new ContextData
use crate::core::control::PipelineControl;
use crate::error::{self, OrkaError, OrkaResult};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
// For potential AnyExtractor update:
// use std::any::{Any, TypeId};
// use std::sync::Arc;

// --- Handler Definition ---

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

// --- Extractor Mechanism for Sub-Contexts SData from TData (using ContextData) ---
// This section needs careful redesign if `Pipeline::on<S>` (for non-conditional sub-contexts)
// is to be maintained and work with ContextData.
//
// Original idea:
//   Extractor: Fn(&mut T) -> OrkaResult<&mut S>
//
// New idea with ContextData:
//   Extractor: Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>>
//   This means SData gets its own independent ContextData wrapper.
//
// Or, if SData is just a part of TData and doesn't need its own independent lock:
//   Extractor: Fn(ContextData<TData>) -> OrkaResult<SomePointerOrRefToPartOfTData>
//   But then the sub-handler for S would still need the main ContextData<TData> to lock.
//   The current pipeline.on<S> signature implies S has its own "context" for its handlers.
//
// Let's assume for now that if `on<S>` is used, `SData` gets its own `ContextData`.

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
}

// Concrete implementation of AnyContextDataExtractor.
pub struct ContextDataExtractorImpl<
  TData: 'static + Send + Sync,
  SData: 'static + Send + Sync, // SData is the underlying data type for the sub-context
> {
  // The user-provided function to get ContextData<SData> from ContextData<TData>.
  extractor_fn: Arc<dyn Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>> + Send + Sync + 'static>,
}

impl<TData: 'static + Send + Sync, SData: 'static + Send + Sync> ContextDataExtractorImpl<TData, SData> {
  pub fn new(f: impl Fn(ContextData<TData>) -> OrkaResult<ContextData<SData>> + Send + Sync + 'static) -> Self {
    Self {
      extractor_fn: Arc::new(f),
    }
  }
}

impl<TData: 'static + Send + Sync, SData: 'static + Send + Sync> AnyContextDataExtractor<TData>
  for ContextDataExtractorImpl<TData, SData>
{
  fn extract_sub_context_data(&self, root_ctx_data: ContextData<TData>) -> OrkaResult<Box<dyn Any + Send>> {
    // Call the user's extractor function.
    let sub_ctx_data: ContextData<SData> = (self.extractor_fn)(root_ctx_data)?;
    // Box it up into Box<dyn Any + Send>.
    Ok(Box::new(sub_ctx_data))
  }

  fn sub_context_data_type_id(&self) -> std::any::TypeId {
    std::any::TypeId::of::<SData>()
  }
}

// Helper function to safely downcast the Box<dyn Any + Send> back to ContextData<SData>.
// This is called within the wrapped Handler<TData> created by `Pipeline::on<SData>`.
pub(crate) fn downcast_context_data<SData: 'static + Send + Sync>(
  any_ctx_data: Box<dyn Any + Send>,
  expected_sdata_type_id: std::any::TypeId, // TypeId of SData (the inner type)
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
      // We don't easily know the "actual" SData type from the Box<dyn Any> holding ContextData<ActualSData>
      // without trying to downcast to ContextData<SomethingElse> first, which is complex.
      // The expected_sdata_type_id comes from the registered extractor.
    });
  }

  match any_ctx_data.downcast::<ContextData<SData>>() {
    Ok(boxed_ctx_data) => Ok(*boxed_ctx_data), // Unbox
    Err(_) => {
      // This should ideally not happen if TypeId matched, unless there's a logic error
      // or the Box<dyn Any> didn't actually contain a ContextData<SData>.
      Err(OrkaError::Internal(format!(
              "Internal type mismatch during ContextData downcast for step '{}'. Expected ContextData<{}> but downcast failed despite TypeId match.",
              step_name,
              std::any::type_name::<SData>()
          )))
    }
  }
}
