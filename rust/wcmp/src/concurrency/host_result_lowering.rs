// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The bound the lowering of a host task's result carries.

use crate::error::Result;
use crate::store::StoreContext;
use crate::value::Val;

/// The bound the lowering of a host task's result carries.
///
/// A host task outlives the call that started it: the guest is told
/// the call started and runs on, and the result crosses into it in
/// whichever later turn the future completes. What the crossing
/// needs — the canon options of the lowering, the memory, the place
/// in it the result goes — is therefore carried in the lowering
/// itself rather than read back from a call that has returned. The
/// lowering is handed the store and what the future produced, and it
/// builds the boundary context of the subtask from what it captured.
///
/// The `Send` half of the bound is the one per-target line, for the
/// reason a queued item's action carries it: a store stays `Send`
/// natively, and in the browser nothing it holds has to be.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostResultLowering<T: 'static>:
    FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + Send + 'static
{
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: 'static, F> HostResultLowering<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + Send + 'static
{
}

/// The bound the lowering of a host task's result carries. See the
/// native definition for what it is and why the `Send` half is
/// absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostResultLowering<T: 'static>:
    FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + 'static
{
}

#[cfg(target_arch = "wasm32")]
impl<T: 'static, F> HostResultLowering<T> for F where
    F: FnOnce(&mut StoreContext<'_, T>, Result<Vec<Val>>) -> Result<()> + 'static
{
}
