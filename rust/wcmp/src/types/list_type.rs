// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
