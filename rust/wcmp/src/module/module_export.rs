// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One export a core module declares.

use super::core_extern_type::CoreExternType;

/// One export of a core module: the name the module publishes the
/// item under and the item's type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModuleExport {
    /// The name the item is exported under.
    pub name: String,
    /// The type of the exported item.
    pub ty: CoreExternType,
}
