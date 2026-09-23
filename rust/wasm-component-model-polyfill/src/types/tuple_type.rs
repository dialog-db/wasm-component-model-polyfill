//! The structural shape of a `tuple<…>` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A heterogeneous, positionally-addressed sequence of value types.
///
/// Two tuple types are structurally equal when they have the same
/// arity and each pair of corresponding element types is itself
/// structurally equal.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct TupleType {
    elements: Vec<ValueType>,
    shape: AbiShape,
}

impl TupleType {
    /// Construct a tuple type from an ordered list of element types.
    pub fn new(elements: impl IntoIterator<Item = ValueType>) -> Self {
        let elements: Vec<ValueType> = elements.into_iter().collect();
        let shape = AbiShape::record(elements.iter());
        Self { elements, shape }
    }

    /// The tuple's element types, in declaration order.
    pub fn elements(&self) -> &[ValueType] {
        &self.elements
    }
}

impl CompoundTypeInternal for TupleType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for TupleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TupleType")
            .field("elements", &self.elements)
            .finish()
    }
}
