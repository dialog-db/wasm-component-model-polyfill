//! The structural shape of a `future<T>` value type.

use super::value_type::ValueType;

/// A future: one value that one party writes and another reads.
///
/// The payload type is the type of the value the future carries. A
/// future with no payload, `future`, carries no value and signals only
/// its completion. Two future types are structurally equal when their
/// payloads agree: both absent, or both present and structurally
/// equal.
///
/// A value of this type is the readable end of a future. Its flat
/// representation is one `i32`, the index of that end in the handle
/// table of the component instance that holds it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FutureType {
    payload: Option<Box<ValueType>>,
}

impl FutureType {
    /// Construct a future type carrying a value of the given payload
    /// type, or no value when the payload is `None`.
    pub fn new(payload: Option<ValueType>) -> Self {
        Self {
            payload: payload.map(Box::new),
        }
    }

    /// The type of the value the future carries, if any.
    pub fn payload(&self) -> Option<&ValueType> {
        self.payload.as_deref()
    }
}
