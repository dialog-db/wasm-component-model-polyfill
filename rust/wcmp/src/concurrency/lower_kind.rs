// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Which lowering a guest called a host function through.

/// Which lowering a guest called a host function through.
///
/// The two differ only in what happens when the host's future is not
/// ready at once. An asynchronous lower hands the call back to the
/// guest as a subtask it can wait on. A synchronous lower has to
/// block the guest thread where it stands, which it does through the
/// suspend seam: the blocked call's own future is polled at every
/// check of the block's condition, so a future that resolves after a
/// few polls resolves inside the block and the call returns its
/// result. Only a future that stays pending fails the call, with the
/// stack-switch cause or with the cannot-block cause, whichever the
/// seam's cause selection names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerKind {
    /// A `canon lower` without `async`: the guest expects the result
    /// when the call returns.
    Sync,
    /// A `canon lower` with `async`: the guest expects a status word
    /// and waits on the subtask it names.
    Async,
}
