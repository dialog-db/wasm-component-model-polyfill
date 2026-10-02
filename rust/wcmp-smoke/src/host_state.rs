// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/// What the host records while the guests run. Every step's store
/// carries one of these, so a host function and a destructor have a
/// place to leave evidence the step reads back.
#[derive(Debug, Default)]
pub struct HostState {
    /// Values guest code handed to the `tally` host function.
    pub tallies: Vec<u32>,
    /// Representations of host resources the guest dropped, in order.
    pub dropped: Vec<u32>,
}
