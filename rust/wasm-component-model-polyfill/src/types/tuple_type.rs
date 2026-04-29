//! The structural shape of a `tuple<…>` value type.

use super::value_type::ValueType;

/// A heterogeneous, positionally-addressed sequence of value types.
///
/// Two tuple types are structurally equal when they have the same
/// arity and each pair of corresponding element types is itself
/// structurally equal.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TupleType {
    elements: Vec<ValueType>,
}

impl TupleType {
    /// Construct a tuple type from an ordered list of element types.
    pub fn new(elements: impl IntoIterator<Item = ValueType>) -> Self {
        Self {
            elements: elements.into_iter().collect(),
        }
    }

    /// The tuple's element types, in declaration order.
    pub fn elements(&self) -> &[ValueType] {
        &self.elements
    }
}
