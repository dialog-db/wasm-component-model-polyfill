// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One import of a module.

use crate::types::ExternType;

/// One import of a module: its two names and its type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImportType {
    module: String,
    name: String,
    ty: ExternType,
}

impl ImportType {
    /// The import `module` `name` of type `ty`.
    pub fn new(module: impl Into<String>, name: impl Into<String>, ty: ExternType) -> Self {
        Self {
            module: module.into(),
            name: name.into(),
            ty,
        }
    }

    /// The module name of the import.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The item name of the import.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The type of the import.
    pub fn ty(&self) -> &ExternType {
        &self.ty
    }
}
