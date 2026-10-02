// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a subject saw once it reached its calls.

use crate::outcome::Outcome;

/// What a subject saw once it reached its calls: how each call ended,
/// and the lines the scenario printed.
///
/// A runner makes every call in the expectations, in order, even after
/// one fails, and reports one outcome per entry. A subject that stopped
/// before its calls has no run; its runner gives it a
/// [`Verdict`](crate::Verdict) of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    /// How each call ended, one per entry of the expectations, in order.
    pub outcomes: Vec<Outcome>,
    /// The lines the scenario printed, in order.
    pub output: Vec<String>,
}
