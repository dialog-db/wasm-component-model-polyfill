// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `Send` natively, and nothing in the browser.

/// `Send` natively, and nothing in the browser.
///
/// A store is `Send` natively, so what it holds and what it polls is
/// too, as in Wasmtime. The browser has one thread, and what a host
/// holds there, a JavaScript promise or a JavaScript object, is not
/// `Send`. The bound the code `wcmp::bindgen!` writes asks of a host
/// future, of the traits an async function's method is in, and of the
/// host data under `require_store_data_send`, is this one, so the code
/// is the same for both targets. Every type that is `Send` implements
/// it natively, and every type implements it in the browser, as
/// [`HostFuture`](crate::HostFuture)'s bound does.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` natively, and nothing in the browser.
///
/// See the native definition: the browser's asks nothing.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}
