// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of a tag.

use crate::types::{FuncType, ValType};

/// The type of a tag: a function type whose parameters are the payload of
/// the tag.
///
/// An exception tag has no results. A tag of stack switching can have
/// results: the values a handler resumes the continuation with.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TagType {
    ty: FuncType,
}

impl TagType {
    /// The tag type of the function type `ty`.
    pub const fn new(ty: FuncType) -> Self {
        Self { ty }
    }

    /// The function type of the tag.
    pub const fn ty(&self) -> &FuncType {
        &self.ty
    }

    /// The types of the parameters of the tag: its payload.
    pub fn params(&self) -> &[ValType] {
        self.ty.params()
    }
}
