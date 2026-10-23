// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The hook a store calls around each entry of guest code.

use std::sync::Arc;

use crate::error::Result;
use crate::store::StoreContextMut;

/// A function the engine calls with the store just before guest code runs
/// and just after it stops, whether it returned, trapped, or suspended.
///
/// The polyfill keeps state that guest code reads without calling the host,
/// such as the context slots of the current thread, in WebAssembly globals.
/// The hook is where it saves and restores that state, because the current
/// thread changes only while the host runs. An error from the hook is the
/// error of the call it runs around.
pub type EntryHook = Arc<dyn Fn(StoreContextMut<'_, ()>) -> Result<()> + Send + Sync>;
