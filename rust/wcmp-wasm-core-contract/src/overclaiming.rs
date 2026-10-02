// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A backend that claims host suspension over one that lacks it.

use wcmp_wasm_core::backend::{Backend, BackendModule, BackendStore, BoxFuture, StoreData};
use wcmp_wasm_core::{Capabilities, Capability, Result};

/// A backend that declares host suspension beside what the backend it
/// wraps declares, and is that backend in everything else.
///
/// The engine checks a capability before it reaches a backend, so a host
/// never reaches a backend's own refusal of host suspension through an
/// engine that tells the truth. A test reaches it through an engine over
/// this backend, whose claim the engine believes.
pub struct Overclaiming<B> {
    inner: B,
}

impl<B: Backend> Overclaiming<B> {
    /// The backend `inner`, claiming host suspension.
    pub fn new(inner: B) -> Self {
        Self { inner }
    }
}

impl<B: Backend> Backend for Overclaiming<B> {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities().with(Capability::HostSuspension)
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        self.inner.compile(bytes)
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        self.inner.compile_sync(bytes)
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        self.inner.new_store(data)
    }
}
