// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a store knew about one resource type at one moment.

use crate::types::ResourceType;

/// What a store knew about one resource type at one moment: whether
/// a destructor was registered for it.
///
/// An instantiation takes one of these for every resource type it is
/// about to register, and hands them back if the plan fails, so that
/// a failed instantiation leaves the store's registrations as it
/// found them.
///
/// A record is taken once per resource type, before the
/// instantiation registers anything for it. A second record of the
/// same type would see the instantiation's own registration and put
/// that back instead of what preceded it.
pub struct ResourceRecord {
    /// The resource type the record is about.
    pub type_id: ResourceType,
    /// Whether the store had a destructor registered for it.
    pub destructor: bool,
}
