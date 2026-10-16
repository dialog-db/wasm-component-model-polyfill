// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the resource of a host handle is.

/// Where a host handle's resource is, as Wasmtime's `ResourceState`
/// says it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostHandleState {
    /// A borrow the host received as a `borrow` parameter, valid until
    /// the host function returns.
    Borrow,
    /// An owned handle in no table: the host made it, or lifted it out
    /// of a guest as an `own`.
    NotInTable,
    /// An owned handle the host lowered into a guest as an `own`. It
    /// lowers no more.
    Taken,
    /// An owned handle in the host table, at an index and a generation:
    /// the host lowered a `borrow` of it, and the table tracks each
    /// borrow.
    Index {
        /// The index of the host table's entry.
        index: u32,
        /// The generation of that entry.
        generation: u32,
    },
}
