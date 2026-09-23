//! The structural shape of a `record` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A record: an ordered, named collection of typed fields.
///
/// Two record types are structurally equal when their field names
/// appear in the same order and each pair of corresponding field
/// types is itself structurally equal.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RecordType {
    fields: Vec<RecordField>,
    shape: AbiShape,
}

impl RecordType {
    /// Construct a record type from an ordered list of fields.
    pub fn new(fields: impl IntoIterator<Item = RecordField>) -> Self {
        let fields: Vec<RecordField> = fields.into_iter().collect();
        let shape = AbiShape::record(fields.iter().map(RecordField::ty));
        Self { fields, shape }
    }

    /// The fields of this record, in declaration order.
    pub fn fields(&self) -> &[RecordField] {
        &self.fields
    }
}

impl CompoundTypeInternal for RecordType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for RecordType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordType")
            .field("fields", &self.fields)
            .finish()
    }
}

/// A single field of a [`RecordType`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecordField {
    name: String,
    ty: ValueType,
}

impl RecordField {
    /// Construct a record field from its name and value type.
    pub fn new(name: impl Into<String>, ty: ValueType) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }

    /// The field's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The field's value type.
    pub fn ty(&self) -> &ValueType {
        &self.ty
    }
}
