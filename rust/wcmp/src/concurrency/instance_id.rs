// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of one component instance's runtime record.

/// The identity of one component instance's runtime record: its
/// index in the store's list of instance records.
///
/// One record exists per component instance of every instantiation
/// in the store. The adapter modules name their instances by the
/// translator's per-instantiation index; the instantiation maps that
/// index onto this store-wide identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InstanceId(u32);

impl InstanceId {
    /// Name the instance record at `index`. Workspace-internal: only
    /// the store's instance list mints one.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.0
    }
}
