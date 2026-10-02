// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One call the Wasmtime run made.

use core::fmt;

use crate::call::Call;
use crate::outcome::Outcome;

/// One call the Wasmtime run made, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The call, as the expectations list it.
    pub call: Call,
    /// How the call ended.
    pub outcome: Outcome,
}

impl fmt::Display for Observation {
    /// The observation as a `call` line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "call {} -> {}", self.call, self.outcome)
    }
}
