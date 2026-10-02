// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `Sync` where the target has threads, and nothing where it does not.

/// `Sync` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, one engine is shared between threads, as Wasmtime's is, so a
/// backend and its compiled modules must be `Sync`. In the browser, a
/// backend holds JavaScript values, which never leave the thread that made
/// them. Every type implements this trait wherever it would implement
/// `Sync`, so a backend never implements it by hand.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, one engine is shared between threads, as Wasmtime's is, so a
/// backend and its compiled modules must be `Sync`. In the browser, a
/// backend holds JavaScript values, which never leave the thread that made
/// them. Every type implements this trait wherever it would implement
/// `Sync`, so a backend never implements it by hand.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}
