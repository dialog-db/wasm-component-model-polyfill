// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `Send` where the target has threads, and nothing where it does not.

/// `Send` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, the engine and its stores move between threads, as Wasmtime's
/// do, so what they hold must be `Send`. In the browser, a backend holds
/// JavaScript values, which never leave the thread that made them, and the
/// polyfill runs on one thread. Every type implements this trait wherever
/// it would implement `Send`, so a backend or a host never implements it by
/// hand.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` on every target but `wasm32`, and no bound on `wasm32`.
///
/// Natively, the engine and its stores move between threads, as Wasmtime's
/// do, so what they hold must be `Send`. In the browser, a backend holds
/// JavaScript values, which never leave the thread that made them, and the
/// polyfill runs on one thread. Every type implements this trait wherever
/// it would implement `Send`, so a backend or a host never implements it by
/// hand.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}
