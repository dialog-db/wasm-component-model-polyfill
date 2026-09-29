//! The structural shape of a `map<K, V>` value type.

use super::tuple_type::TupleType;
use super::value_type::ValueType;

/// An association from keys of one type to values of another.
///
/// The canonical ABI represents a map exactly as a `list<tuple<K, V>>`
/// of its entries, so a map's layout, alignment, and flat form are
/// those of that list. Two map types are structurally equal when
/// their key types and their value types are.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MapType {
    key: Box<ValueType>,
    value: Box<ValueType>,
}

impl MapType {
    /// Construct a map type with the given key and value types.
    pub fn new(key: ValueType, value: ValueType) -> Self {
        Self {
            key: Box::new(key),
            value: Box::new(value),
        }
    }

    /// The map's key type.
    pub fn key(&self) -> &ValueType {
        &self.key
    }

    /// The map's value type.
    pub fn value(&self) -> &ValueType {
        &self.value
    }

    /// The type of one entry as the canonical ABI lays it out: the
    /// tuple of the key and the value.
    pub fn entry(&self) -> ValueType {
        ValueType::Tuple(TupleType::new([(*self.key).clone(), (*self.value).clone()]))
    }
}
