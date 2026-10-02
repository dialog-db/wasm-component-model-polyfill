// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of one error-context record.

/// The identity of one error context: its index in the store's table
/// of error-context records, together with the generation that slot
/// carried when the record was inserted.
///
/// A guest that holds an error context holds a handle-table entry
/// that carries this identity; the identity itself never reaches a
/// guest. The generation keeps an identity minted for a record that
/// is gone from naming the record that takes its index next, under
/// the rule [`WaitableSetId`](super::WaitableSetId) states.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ErrorContextId {
    index: u32,
    generation: u32,
}

impl ErrorContextId {
    /// Name the error-context record at `index` of generation
    /// `generation`. Workspace-internal: only the store's table of
    /// error-context records mints one.
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
