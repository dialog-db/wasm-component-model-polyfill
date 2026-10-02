// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of one thread record.

/// The identity of one thread record: its index in the store's table
/// of threads, together with the generation that slot carried when
/// the record was inserted.
///
/// Every task has at least one thread, its implicit thread. A thread
/// is one guest execution: it carries the context slots
/// `context.get` and `context.set` read and write, and the readiness
/// condition it waits on while it is suspended.
///
/// Indices are per store and a freed index is handed out again, so
/// an index on its own names one thread only for as long as that
/// thread's record lives. The generation is what makes the identity
/// outlive the index safely: removing a thread record advances its
/// slot's generation, so an identity minted for a thread that has
/// ended matches no record at all, and in particular never matches
/// the thread that takes the index next. A context slot written
/// through a stale identity would otherwise land on another call's
/// thread.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadId {
    index: u32,
    generation: u32,
}

impl ThreadId {
    /// Name the thread record at `index` of generation `generation`.
    /// Workspace-internal: only the store's thread table mints one.
    pub const fn new(index: u32, generation: u32) -> Self {
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
