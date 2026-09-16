//! The structural shape of a fixed-length `list<T, N>` value type.

use super::value_type::ValueType;

/// A list of exactly `N` values of one element type.
///
/// The canonical ABI lays a fixed-length list out inline, element
/// after element, as it lays out a tuple of `N` copies of the element
/// type: no pointer and length pair, and `N` times the element's flat
/// slots when the whole fits in the flat form. Two fixed-length list
/// types are structurally equal when their element types and their
/// lengths are.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FixedLengthListType {
    element: Box<ValueType>,
    length: u32,
}

impl FixedLengthListType {
    /// Construct a fixed-length list type with the given element
    /// type and length.
    pub fn new(element: ValueType, length: u32) -> Self {
        Self {
            element: Box::new(element),
            length,
        }
    }

    /// The list's element type.
    pub fn element(&self) -> &ValueType {
        &self.element
    }

    /// The number of elements every value of the type holds.
    pub fn length(&self) -> u32 {
        self.length
    }
}
