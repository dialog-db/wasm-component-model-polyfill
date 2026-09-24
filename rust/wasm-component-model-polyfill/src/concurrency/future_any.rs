//! The readable end of a future, as an untyped value carries it.

use crate::internal::FutureAnyInternal;
use crate::types::ValueType;

use super::end_id::EndId;

/// The readable end of a future, as [`Val::Future`](crate::Val::Future)
/// carries it. The name is Wasmtime's.
///
/// It holds the end and the type of the value the future carries.
/// A [`FutureReader`](super::FutureReader) becomes one when it
/// crosses as a [`Val`](crate::Val), which is how a typed host
/// function hands a future to a guest, and lowering one into a guest
/// enters its end in the guest's handle table after checking that the
/// guest's type carries the same payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FutureAny {
    end: EndId,
    /// Boxed, so that a `Val` that carries one stays the size it is
    /// without: the copy budget charges that size per element.
    payload: Option<Box<ValueType>>,
}

impl FutureAnyInternal for FutureAny {
    fn new(end: EndId, payload: Option<ValueType>) -> Self {
        Self {
            end,
            payload: payload.map(Box::new),
        }
    }

    fn end(&self) -> EndId {
        self.end
    }

    fn payload(&self) -> Option<&ValueType> {
        self.payload.as_deref()
    }
}
