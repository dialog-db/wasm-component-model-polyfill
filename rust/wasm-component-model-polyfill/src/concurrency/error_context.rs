//! An error context, as a typed entry carries it.

use crate::internal::ErrorContextInternal;

use super::error_context_id::ErrorContextId;

/// An error context (`error-context`), as a typed host function or a
/// typed call carries it. The name is Wasmtime's.
///
/// It is the typed face of [`ErrorContextAny`](crate::ErrorContextAny)
/// and follows the same rules: it has no operation, a lift to the host
/// marks its record host-held so the record stays until the store
/// drops, and a lower into a guest gives the guest a handle of its
/// own. It implements [`ComponentValue`](crate::ComponentValue), which
/// converts it to and from a
/// [`Val::ErrorContext`](crate::Val::ErrorContext) and lifts and
/// lowers it as a typed value.
#[derive(Debug)]
pub struct ErrorContext {
    context: ErrorContextId,
}

impl ErrorContextInternal for ErrorContext {
    fn new(context: ErrorContextId) -> Self {
        Self { context }
    }

    fn context(&self) -> ErrorContextId {
        self.context
    }
}
