// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of an extern.

use crate::types::{FuncType, GlobalType, MemoryType, TableType, TagType};

/// The type of an import or an export of a module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExternType {
    /// A function.
    Func(FuncType),
    /// A global.
    Global(GlobalType),
    /// A table.
    Table(TableType),
    /// A memory.
    Memory(MemoryType),
    /// A tag.
    Tag(TagType),
}
