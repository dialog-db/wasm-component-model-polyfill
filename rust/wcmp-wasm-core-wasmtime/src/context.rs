// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the backend runs a store operation on.

use crate::state::State;

/// A Wasmtime context the backend runs a store operation on: the store
/// itself, or the `Caller` a host function receives while a guest runs in
/// the store.
///
/// Both reach the [`State`] of the store. Wasmtime's own `AsContextMut`
/// lends the state mutably only for as long as a temporary context lives,
/// so each implementation reaches it directly.
pub trait Context: wasmtime::AsContextMut<Data = State> + Send {
    /// The state of the store.
    fn state(&self) -> &State;

    /// The state of the store, mutably.
    fn state_mut(&mut self) -> &mut State;
}

impl Context for wasmtime::Store<State> {
    fn state(&self) -> &State {
        self.data()
    }

    fn state_mut(&mut self) -> &mut State {
        self.data_mut()
    }
}

impl Context for wasmtime::Caller<'_, State> {
    fn state(&self) -> &State {
        self.data()
    }

    fn state_mut(&mut self) -> &mut State {
        self.data_mut()
    }
}
