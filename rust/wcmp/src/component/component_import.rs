// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A single declared import of a component.

use super::extern_type::ExternType;
use super::external_name::ExternalName;

/// One declared import of a parsed component.
///
/// An import pairs the name under which the host must supply the
/// item with the typed shape that item must present. The name is an
/// [`ExternalName`] — either a parsed interface identifier or a
/// plain string — so a downstream tool can group imports by package
/// and interface without re-parsing the raw binary string.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComponentImport {
    /// The name under which this import is declared.
    pub name: ExternalName,
    /// The extern type the host must satisfy.
    pub ty: ExternType,
}
