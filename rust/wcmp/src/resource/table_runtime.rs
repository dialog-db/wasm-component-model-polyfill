// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One resource type's view of a component instance's handle table.

use super::identity::ResourceTypeId;
use super::table_id::TableId;

/// One resource type's view of a component instance's handle table,
/// as the canonical ABI addresses it: the table (one per instance,
/// shared by every resource type the instance uses), the resource
/// type, and whether the instance is the one that defines it. The
/// defining instance handles its own resource's reps directly on the
/// lower side only: a borrow lowered into it is the rep itself rather
/// than a table entry, while a borrow lifted out of it reads the entry
/// its index names, as any other instance's does.
#[derive(Clone, Copy, Debug)]
pub struct ResourceTableRuntime {
    /// The table the instance keeps for the resource.
    pub table: TableId,
    /// The identity of the resource type the table holds.
    pub type_id: ResourceTypeId,
    /// The resource's index in the component's resource list.
    pub resource_index: usize,
    /// Whether the table's instance defines the resource.
    pub defining: bool,
    /// Whether a component defines the resource at all (`true`), or
    /// the host registered it. Read for the trap message.
    pub guest_defined: bool,
}
