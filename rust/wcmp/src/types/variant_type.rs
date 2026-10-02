// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structural shape of a `variant` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A variant: a tagged union with named cases that may carry a
/// payload.
///
/// Two variant types are structurally equal when their cases appear
/// in the same order and each pair of corresponding case names and
/// optional payloads is itself structurally equal.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct VariantType {
    cases: Vec<VariantCase>,
    shape: AbiShape,
}

impl VariantType {
    /// Construct a variant type from an ordered list of cases.
    pub fn new(cases: impl IntoIterator<Item = VariantCase>) -> Self {
        let cases: Vec<VariantCase> = cases.into_iter().collect();
        let shape = AbiShape::variant(cases.iter().map(VariantCase::payload));
        Self { cases, shape }
    }

    /// The cases of this variant, in declaration order.
    pub fn cases(&self) -> &[VariantCase] {
        &self.cases
    }
}

impl CompoundTypeInternal for VariantType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for VariantType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VariantType")
            .field("cases", &self.cases)
            .finish()
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
