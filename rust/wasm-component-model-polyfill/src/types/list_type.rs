//! The structural shape of a `list<T>` value type.

use super::value_type::ValueType;

/// A homogeneous list of values of a single element type.
///
/// Two list types are structurally equal when their element types
/// are structurally equal.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListType {
    element: Box<ValueType>,
}

impl ListType {
    /// Construct a list type with the given element type.
    pub fn new(element: ValueType) -> Self {
        Self {
            element: Box::new(element),
        }
    }

    /// The list's element type.
    pub fn element(&self) -> &ValueType {
        &self.element
    }
}
