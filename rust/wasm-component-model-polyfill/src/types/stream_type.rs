//! The structural shape of a `stream<T>` value type.

use super::value_type::ValueType;

/// A stream: a sequence of values that one party writes and another
/// reads.
///
/// The payload type is the type of each value the stream carries. A
/// stream with no payload, `stream`, carries no values and signals
/// only through its readable and writable ends. Two stream types are
/// structurally equal when their payloads agree: both absent, or both
/// present and structurally equal.
///
/// A value of this type is the readable end of a stream. Its flat
/// representation is one `i32`, the index of that end in the handle
/// table of the component instance that holds it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StreamType {
    payload: Option<Box<ValueType>>,
}

impl StreamType {
    /// Construct a stream type carrying values of the given payload
    /// type, or no values when the payload is `None`.
    pub fn new(payload: Option<ValueType>) -> Self {
        Self {
            payload: payload.map(Box::new),
        }
    }

    /// The type of each value the stream carries, if any.
    pub fn payload(&self) -> Option<&ValueType> {
        self.payload.as_deref()
    }
}
