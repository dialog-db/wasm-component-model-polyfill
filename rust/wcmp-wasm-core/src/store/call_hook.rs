// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The crossings between the host and guest code that a store's call hook
//! sees.

/// A crossing between the host and guest code, which the engine passes to
/// the hook a host sets with [`Store::call_hook`](crate::Store::call_hook),
/// as Wasmtime's `CallHook` is.
///
/// The engine names a crossing just before control passes:
///
/// - [`CallingWasm`](Self::CallingWasm) before each stretch of guest code
///   the host runs: a call, the start of a resumable call, a resumption,
///   each wait for a resumable call to stop, and each instantiation.
/// - [`ReturningFromWasm`](Self::ReturningFromWasm) after each of those,
///   whether the guest returned, trapped, or suspended.
/// - [`CallingHost`](Self::CallingHost) before the body of a host function
///   that a guest called.
/// - [`ReturningFromHost`](Self::ReturningFromHost) after that body. A
///   suspending host function that answers "not yet" does not return to the
///   guest, so it has no such crossing. The stretch it suspends ends with
///   [`ReturningFromWasm`](Self::ReturningFromWasm).
///
/// Two crossings do not pair as they would in Wasmtime:
///
/// - Where the future of an asynchronous stretch drops before it ends, as
///   the host may drop the wait of a resumable call, the stretch has no
///   [`ReturningFromWasm`](Self::ReturningFromWasm). A hook that counts
///   depth must not rely on one.
/// - The engine crosses around every instantiation, because it does not
///   know whether the module has a start function. Wasmtime crosses only
///   where the instance runs one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallHook {
    /// The host calls guest code.
    CallingWasm,
    /// Guest code returns to the host.
    ReturningFromWasm,
    /// Guest code calls a host function.
    CallingHost,
    /// A host function returns to guest code.
    ReturningFromHost,
}

impl CallHook {
    /// Whether the host runs next.
    pub fn entering_host(&self) -> bool {
        matches!(self, Self::ReturningFromWasm | Self::CallingHost)
    }

    /// Whether guest code runs next.
    pub fn exiting_host(&self) -> bool {
        matches!(self, Self::ReturningFromHost | Self::CallingWasm)
    }
}
