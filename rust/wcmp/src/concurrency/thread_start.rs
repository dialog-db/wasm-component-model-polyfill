// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What an explicit thread runs when it starts.

use crate::runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

/// What an explicit thread runs when it starts: the start function
/// `thread.new-indirect` read out of its table, and the context value
/// the guest passed it.
///
/// The function is read when the thread is created, as the reference
/// reads it, so a guest that changes the table entry afterwards does
/// not change what the thread runs. The record lives on the thread
/// until the thread starts, because the start is a later turn's work.
pub struct ThreadStart {
    /// The start function, of type `(i32) -> ()` or, in a 64-bit
    /// memory, `(i64) -> ()`.
    pub function: RuntimeFunc,
    /// The context value, an `i32` or an `i64` to match the function.
    pub context: RuntimeVal,
}
