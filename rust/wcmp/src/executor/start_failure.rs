// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where a start item leaves the failure that belongs to the caller.

use std::sync::{Arc, Mutex};

use crate::error::Error;

/// Where a start item leaves the failure that belongs to the caller:
/// the lowering of the arguments, a trap in the callee's core
/// function, or the status word that function returned.
///
/// The item outlives the trampoline's frame when the entry gate
/// holds it, so both sides hold the slot. A synchronous lower is
/// what takes one: its trampoline blocks for the callee's result, a
/// failure must not end the turn that ran the item — the turn can be
/// a nested one the block itself is running — and the trampoline
/// reads the slot when the block returns.
pub type StartFailure = Arc<Mutex<Option<Error>>>;
