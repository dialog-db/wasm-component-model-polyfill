// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The errors of the runtime layer, and the kinds of trap.

#[allow(clippy::module_inception)]
mod error;
mod trap_kind;

pub use error::{Error, Result};
pub use trap_kind::TrapKind;
