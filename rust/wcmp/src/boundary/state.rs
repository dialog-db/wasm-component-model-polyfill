// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::cell::RefCell;

#[cfg(feature = "boundary-counters")]
use super::BoundaryCounters;

/// The counters, and the clock of the boundary time: the time the
/// segment that runs now started, if one runs.
#[cfg(feature = "boundary-counters")]
#[derive(Default)]
pub struct State {
    /// What the counters hold.
    pub counters: BoundaryCounters,
    /// When the boundary segment that runs now started.
    pub running_since: Option<f64>,
}

#[cfg(feature = "boundary-counters")]
thread_local! {
    /// This thread's counters and clock.
    pub static STATE: RefCell<State> = RefCell::new(State::default());
}
