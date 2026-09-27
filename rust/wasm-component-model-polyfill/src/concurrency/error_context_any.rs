//! An error context, as an untyped value carries it.

use crate::internal::ErrorContextAnyInternal;

use super::error_context_id::ErrorContextId;

/// An error context, as [`Val::ErrorContext`](crate::Val::ErrorContext)
/// carries it. The name is Wasmtime's.
///
/// It names one error-context record of the store: the debug message
/// a guest gave `error-context.new`. It has no operation. The value
/// carries an error context from one component to another, as the
/// payload of a stream or a future that a copy moves between them,
/// and a value of it reaches the host in no other way. A lift or a
/// lower of an `error-context` between the host and a guest fails
/// with [`Error::Unsupported`](crate::Error::Unsupported).
///
/// The value names its record by an index and a generation, and
/// cloning it copies the name, not the record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorContextAny {
    context: ErrorContextId,
}

impl ErrorContextAnyInternal for ErrorContextAny {
    fn new(context: ErrorContextId) -> Self {
        Self { context }
    }

    fn context(&self) -> ErrorContextId {
        self.context
    }
}
