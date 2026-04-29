//! The structural shape of an `option<T>` value type.

use super::value_type::ValueType;

/// A value that may be present (`some`) or absent (`none`).
///
/// Two option types are structurally equal when their payload types
/// are structurally equal.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OptionType {
    payload: Box<ValueType>,
}

impl OptionType {
    /// Construct an option type carrying the given payload type.
    pub fn new(payload: ValueType) -> Self {
        Self {
            payload: Box::new(payload),
        }
    }

    /// The option's payload type — the value type carried by `some`.
    pub fn payload(&self) -> &ValueType {
        &self.payload
    }
}
