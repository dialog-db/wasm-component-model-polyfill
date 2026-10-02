// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structural shape of a fixed-length `list<T, N>` value type.

use std::fmt;

use super::value_type::ValueType;
use crate::abi::shape::AbiShape;
use crate::internal::CompoundTypeInternal;

/// A list of exactly `N` values of one element type.
///
/// The canonical ABI lays a fixed-length list out inline, element
/// after element, as it lays out a tuple of `N` copies of the element
/// type: no pointer and length pair, and `N` times the element's flat
/// slots when the whole fits in the flat form. Two fixed-length list
/// types are structurally equal when their element types and their
/// lengths are.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct FixedLengthListType {
    element: Box<ValueType>,
    length: u32,
    shape: AbiShape,
}

impl FixedLengthListType {
    /// Construct a fixed-length list type with the given element
    /// type and length.
    pub fn new(element: ValueType, length: u32) -> Self {
        let shape = AbiShape::fixed_length_list(&element, length);
        Self {
            element: Box::new(element),
            length,
            shape,
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

impl CompoundTypeInternal for FixedLengthListType {
    fn abi_shape(&self) -> &AbiShape {
        &self.shape
    }
}

impl fmt::Debug for FixedLengthListType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FixedLengthListType")
            .field("element", &self.element)
            .field("length", &self.length)
            .finish()
    }
}
