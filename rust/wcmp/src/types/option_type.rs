//! The structural shape of an `option<T>` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A value that may be present (`some`) or absent (`none`).
///
/// Two option types are structurally equal when their payload types
/// are structurally equal.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct OptionType {
    payload: Box<ValueType>,
    shape: AbiShape,
}

impl OptionType {
    /// Construct an option type carrying the given payload type.
    pub fn new(payload: ValueType) -> Self {
        let shape = AbiShape::variant([None, Some(&payload)].into_iter());
        Self {
            payload: Box::new(payload),
            shape,
        }
    }

    /// The option's payload type — the value type carried by `some`.
    pub fn payload(&self) -> &ValueType {
        &self.payload
    }
}

impl CompoundTypeInternal for OptionType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OptionType")
            .field("payload", &self.payload)
            .finish()
    }
}
