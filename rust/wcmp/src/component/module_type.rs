// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structural shape of a core module type at the component
//! boundary.

use crate::module::{ModuleExport, ModuleImport};

/// A core module type: the imports a module of this type asks for
/// and the exports it provides.
///
/// A component import or export of a core module carries a module
/// type. The type is the contract a supplied module must satisfy
/// (it must provide every listed export with a matching type and ask
/// for no import outside the listed ones) and the shape a host can
/// count on when it takes an exported module.
///
/// Two module types are structurally equal when their imports and
/// exports appear in the same order with the same names and types.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModuleType {
    /// The declared imports, in declaration order.
    pub imports: Vec<ModuleImport>,
    /// The declared exports, in declaration order.
    pub exports: Vec<ModuleExport>,
}
