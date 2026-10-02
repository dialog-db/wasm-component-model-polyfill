// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of a table.

use crate::types::RefType;

/// The type of a table: the type of its elements, its limits, and whether
/// it is addressed with 64-bit numbers.
///
/// A table addressed with 64-bit numbers needs the
/// [`memory64`](crate::Capability::Memory64) capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableType {
    element: RefType,
    minimum: u64,
    maximum: Option<u64>,
    is_64: bool,
}

impl TableType {
    /// A table of `element` references addressed with 32-bit numbers.
    pub const fn new(element: RefType, minimum: u32, maximum: Option<u32>) -> Self {
        Self {
            element,
            minimum: minimum as u64,
            maximum: match maximum {
                Some(maximum) => Some(maximum as u64),
                None => None,
            },
            is_64: false,
        }
    }

    /// A table of `element` references addressed with 64-bit numbers.
    pub const fn new64(element: RefType, minimum: u64, maximum: Option<u64>) -> Self {
        Self {
            element,
            minimum,
            maximum,
            is_64: true,
        }
    }

    /// The type of the elements of the table.
    pub const fn element(&self) -> &RefType {
        &self.element
    }

    /// The least number of elements of the table.
    pub const fn minimum(&self) -> u64 {
        self.minimum
    }

    /// The greatest number of elements of the table, where it has one.
    pub const fn maximum(&self) -> Option<u64> {
        self.maximum
    }

    /// Whether the table is addressed with 64-bit numbers.
    pub const fn is_64(&self) -> bool {
        self.is_64
    }
}
