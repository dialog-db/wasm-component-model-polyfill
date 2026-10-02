// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of one task record.

/// The identity of one task record: its index in the store's table
/// of tasks, together with the generation that slot carried when the
/// record was inserted.
///
/// Indices are per store and a freed index is handed out again, so
/// an index on its own names one task only for as long as that
/// task's record lives. The generation is what makes the identity
/// outlive the index safely: removing a task record advances its
/// slot's generation, so an identity minted for a task that has
/// ended matches no record at all, and in particular never matches
/// the task that takes the index next.
///
/// That matters because a borrow entry in a handle table names the
/// task the borrow is owed to by this identity, and an entry can
/// outlive its task: a call that fails with borrows outstanding
/// leaves its entries in the table. Dropping such an entry looks its
/// task up and finds nothing, which fails the drop, rather than
/// decrementing the borrow count of whichever task took the index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId {
    index: u32,
    generation: u32,
}

impl TaskId {
    /// Name the task record at `index` of generation `generation`.
    /// Workspace-internal: only the store's task table mints one.
    pub fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation this identity names. A record table slot
    /// matches the identity only while it carries this generation.
    pub fn generation(self) -> u32 {
        self.generation
    }
}
