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
use crate::store::StoreId;

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
}

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
