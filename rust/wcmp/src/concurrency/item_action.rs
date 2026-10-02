// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The bound the action of a queued item carries.

use crate::error::Result;
use crate::store::StoreContext;

/// The bound the action of a queued item carries.
///
/// An item outlives the driver that queued it: dropping a driver's
/// future cancels nothing, so the action sits in the store until
/// some turn runs it. The action is therefore `'static`, and it
/// reaches the store it runs against through the argument the
/// scheduler hands it rather than by borrowing one.
///
/// An action answers with a result. Almost every one of them
/// succeeds whatever happens, because what it produces it leaves in
/// the store for whoever waits on it; the failure of the call an
/// item runs is part of what it leaves. The result is for the
/// failures that belong to no caller — a record that vanished
/// between the turn that queued the item and the turn that ran it —
/// which have nowhere else to go. Such a failure ends the turn and
/// the driver that polled it sees it.
///
/// The `Send` half of the bound is the one per-target line. A store
/// stays `Send` natively, so an action queued into it must be `Send`
/// too. In the browser the bound is absent: the whole polyfill runs
/// on one thread there, and an action that captures a JavaScript
/// value is not `Send` and does not need to be.
#[cfg(not(target_arch = "wasm32"))]
pub trait ItemAction<T: 'static>:
    FnOnce(&mut StoreContext<'_, T>) -> Result<()> + Send + 'static
{
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: 'static, F> ItemAction<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>) -> Result<()> + Send + 'static
{
}

/// The bound the action of a queued item carries. See the native
/// definition for what it is and why the `Send` half is absent here.
#[cfg(target_arch = "wasm32")]
pub trait ItemAction<T: 'static>: FnOnce(&mut StoreContext<'_, T>) -> Result<()> + 'static {}

#[cfg(target_arch = "wasm32")]
impl<T: 'static, F> ItemAction<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>) -> Result<()> + 'static
{
}
