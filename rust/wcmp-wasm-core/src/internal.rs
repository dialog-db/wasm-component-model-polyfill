// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The seams the crate reaches through on its own public types.
//!
//! Every method a public type offers the crate and not its callers is a
//! method of a trait here. The module is private, so no caller outside the
//! crate can name a trait of it, and none of these methods is part of the
//! public API.

use crate::contract::{Backend, BackendModule, BackendStore};
use crate::engine::Engine;
use crate::error::Result;
use crate::store::{CallHook, StoreId};

/// What the crate reaches on an [`Engine`].
pub trait EngineInternal {
    /// The backend the engine holds.
    fn backend(&self) -> &dyn Backend;
}

/// What the crate reaches on a [`Module`](crate::Module).
pub trait ModuleInternal {
    /// The module as its backend holds it.
    fn backend_module(&self) -> &dyn BackendModule;
}

/// How the crate numbers its stores.
pub trait StoreIdInternal {
    /// A number no other store of this process has.
    fn allocate() -> StoreId;
}

/// What the crate reaches on a [`StoreData`](crate::backend::StoreData).
pub trait StoreDataInternal: Sized {
    /// The data of a new store of `engine` whose host data is `value`.
    fn new<T: crate::contract::MaybeSend + 'static>(engine: Engine, value: T) -> Self;

    /// The host data, which is a `T` for every store of `T`.
    fn user<T: 'static>(&self) -> &T;

    /// The host data, mutably.
    fn user_mut<T: 'static>(&mut self) -> &mut T;

    /// Take the store's call hook out, so that it can run with the store.
    fn take_call_hook(&mut self) -> Option<ErasedCallHook>;

    /// Set the store's call hook.
    fn set_call_hook(&mut self, hook: ErasedCallHook);

    /// Put back the call hook that ran, unless the hook set another while
    /// it ran.
    fn restore_call_hook(&mut self, hook: ErasedCallHook);
}

/// A store's call hook, with the type of the host's data erased.
pub type ErasedCallHook =
    Box<dyn FnMut(&mut dyn BackendStore, CallHook) -> Result<()> + Send + Sync>;

/// What the crate reaches on a [`StoreContext`](crate::StoreContext).
pub trait StoreContextInternal<'a> {
    /// A context over the store `store`.
    fn from_backend(store: &'a dyn BackendStore) -> Self;

    /// The store as its backend holds it.
    fn backend(&self) -> &'a dyn BackendStore;
}

/// What the crate reaches on a [`StoreContextMut`](crate::StoreContextMut).
pub trait StoreContextMutInternal<'a> {
    /// A context over the store `store`.
    fn from_backend(store: &'a mut dyn BackendStore) -> Self;

    /// The store as its backend holds it.
    fn backend_mut(&mut self) -> &mut dyn BackendStore;
}

/// What the crate reaches on a [`Caller`](crate::Caller).
pub trait CallerInternal<'a> {
    /// The caller over the store `store`, which a guest runs in.
    fn from_backend(store: &'a mut dyn BackendStore) -> Self;
}

/// Call the call hook of `store` with the crossing `kind`, if the store has
/// a hook.
///
/// The hook is out of the store while it runs, as in Wasmtime, so a
/// crossing the hook itself makes does not call it again, and a hook it
/// sets takes its place. A store whose
/// backend refuses its data, as the browser's does for a store the host
/// forgot, calls no hook: the call that follows meets the refusal, with the
/// error the backend gives it.
pub fn call_hook(store: &mut dyn BackendStore, kind: CallHook) -> Result<()> {
    let Some(mut hook) = store
        .try_data_mut()
        .and_then(StoreDataInternal::take_call_hook)
    else {
        return Ok(());
    };
    let outcome = hook(&mut *store, kind);
    if let Some(data) = store.try_data_mut() {
        data.restore_call_hook(hook);
    }
    outcome
}
