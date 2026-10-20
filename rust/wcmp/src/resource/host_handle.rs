// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The handle to a host resource that `Resource<T>` and
//! `ResourceDynamic` share, with Wasmtime's four states
//! (`crates/wasmtime/src/runtime/component/resources/host.rs` at
//! `v50.0.0`).

use std::sync::atomic::{AtomicU32, Ordering};

use super::host_handle_state::HostHandleState;

/// The state of a host handle, which its lowering changes through a
/// shared reference, as Wasmtime's `AtomicResourceState` does: the
/// index and the generation of the host table's entry, or a sentinel
/// generation for each other state.
#[derive(Debug)]
pub struct HostHandle {
    rep: u32,
    index: AtomicU32,
    generation: AtomicU32,
}

/// The sentinel generations of the states that are not an index. A
/// host table generation never reaches them.
const BORROW: u32 = u32::MAX;
const NOT_IN_TABLE: u32 = u32::MAX - 1;
const TAKEN: u32 = u32::MAX - 2;

impl HostHandle {
    /// An owned handle in no table, of the resource `rep`.
    pub fn own(rep: u32) -> Self {
        Self::with(rep, HostHandleState::NotInTable)
    }

    /// A borrow of the resource `rep`.
    pub fn borrow(rep: u32) -> Self {
        Self::with(rep, HostHandleState::Borrow)
    }

    fn with(rep: u32, state: HostHandleState) -> Self {
        let (index, generation) = encode(state);
        Self {
            rep,
            index: AtomicU32::new(index),
            generation: AtomicU32::new(generation),
        }
    }

    /// The resource's rep.
    pub fn rep(&self) -> u32 {
        self.rep
    }

    /// Where the resource is now.
    pub fn state(&self) -> HostHandleState {
        decode(
            self.index.load(Ordering::Relaxed),
            self.generation.load(Ordering::Relaxed),
        )
    }

    /// Record that the resource is now `state`.
    pub fn set(&self, state: HostHandleState) {
        let (index, generation) = encode(state);
        self.index.store(index, Ordering::Relaxed);
        self.generation.store(generation, Ordering::Relaxed);
    }

    /// Whether the handle is owned rather than a borrow.
    pub fn owned(&self) -> bool {
        !matches!(self.state(), HostHandleState::Borrow)
    }
}

fn encode(state: HostHandleState) -> (u32, u32) {
    match state {
        HostHandleState::Borrow => (0, BORROW),
        HostHandleState::NotInTable => (0, NOT_IN_TABLE),
        HostHandleState::Taken => (0, TAKEN),
        HostHandleState::Index { index, generation } => (index, generation),
    }
}

fn decode(index: u32, generation: u32) -> HostHandleState {
    match generation {
        BORROW => HostHandleState::Borrow,
        NOT_IN_TABLE => HostHandleState::NotInTable,
        TAKEN => HostHandleState::Taken,
        generation => HostHandleState::Index { index, generation },
    }
}
