// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structural shape of an `enum` value type.

/// A tag-only enumeration: a closed set of named, payload-free
/// discriminants.
///
/// `enum` differs from a payloadless [`VariantType`] only in spelling
/// at the binary level; both reduce to a tagged choice between named
/// arms and the polyfill keeps them as distinct shapes so a parsed
/// component round-trips with the same shape its WIT source declared.
///
/// Two enum types are structurally equal when their case names
/// appear in the same order.
///
/// [`VariantType`]: super::variant_type::VariantType
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EnumType {
    cases: Vec<String>,
}

impl EnumType {
    /// Construct an enum type from an ordered list of case names.
    pub fn new(cases: impl IntoIterator<Item = String>) -> Self {
        Self {
            cases: cases.into_iter().collect(),
        }
    }

    /// The case names, in declaration order.
    pub fn cases(&self) -> &[String] {
        &self.cases
    }
}
