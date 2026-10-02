// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Write access to a store, from whatever holds it.

use crate::store::{AsContext, StoreContextMut};

/// A type that gives write access to a store: a [`Store`](crate::Store), a
/// [`Caller`](crate::Caller), or a mutable context, or a mutable reference
/// to one.
///
/// Every method that changes a store, or runs a guest in it, takes
/// `impl AsContextMut`, as Wasmtime's methods do.
pub trait AsContextMut: AsContext {
    /// Write access to the store.
    fn as_context_mut(&mut self) -> StoreContextMut<'_, Self::Data>;
}

impl<C: AsContextMut + ?Sized> AsContextMut for &mut C {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, Self::Data> {
        C::as_context_mut(*self)
    }
}
