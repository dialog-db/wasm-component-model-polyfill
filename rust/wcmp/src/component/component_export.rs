// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A single declared export of a component.

use super::extern_type::ExternType;
use super::external_name::ExternalName;

/// One declared export of a parsed component.
///
/// An export pairs the name under which the component publishes an
/// item with the typed shape that item presents to a host.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComponentExport {
    /// The name under which this export is declared.
    pub name: ExternalName,
    /// The extern type the component publishes.
    pub ty: ExternType,
}
