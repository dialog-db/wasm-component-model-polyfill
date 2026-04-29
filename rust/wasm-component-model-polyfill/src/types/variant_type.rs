//! The structural shape of a `variant` value type.

use super::value_type::ValueType;

/// A variant: a tagged union with named cases that may carry a
/// payload.
///
/// Two variant types are structurally equal when their cases appear
/// in the same order and each pair of corresponding case names and
/// optional payloads is itself structurally equal.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VariantType {
    cases: Vec<VariantCase>,
}

impl VariantType {
    /// Construct a variant type from an ordered list of cases.
    pub fn new(cases: impl IntoIterator<Item = VariantCase>) -> Self {
        Self {
            cases: cases.into_iter().collect(),
        }
    }

    /// The cases of this variant, in declaration order.
    pub fn cases(&self) -> &[VariantCase] {
        &self.cases
    }
}

/// A single case of a [`VariantType`].
///
/// A case carries a name and, optionally, a payload type. A case
/// without a payload represents a tag-only arm.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VariantCase {
    name: String,
    payload: Option<ValueType>,
}

impl VariantCase {
    /// Construct a variant case from its name and optional payload.
    pub fn new(name: impl Into<String>, payload: Option<ValueType>) -> Self {
        Self {
            name: name.into(),
            payload,
        }
    }

    /// The case's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The case's payload type, if any.
    pub fn payload(&self) -> Option<&ValueType> {
        self.payload.as_ref()
    }
}
