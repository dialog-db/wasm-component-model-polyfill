// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::cell::RefCell;

#[cfg(feature = "boundary-counters")]
use super::BoundaryCounters;
#[cfg(feature = "boundary-counters")]
use super::open_scope::OpenScope;

/// The counters, and the clock of the boundary time: the time the
/// segment that runs now started, if one runs.
#[cfg(feature = "boundary-counters")]
#[derive(Default)]
pub struct State {
    /// What the counters hold.
    pub counters: BoundaryCounters,
    /// When the boundary segment that runs now started, or when the
    /// clock was last settled into a scope while it ran.
    pub running_since: Option<f64>,
    /// When the host code that runs now started, if host code runs and no
    /// boundary segment inside it does, or when the clock was last
    /// settled into a scope while it ran.
    pub host_since: Option<f64>,
    /// The scopes open now, the innermost last. Each count goes to the
    /// innermost one as well as to the thread's counters.
    pub scopes: Vec<OpenScope>,
    /// The identity the next scope opened takes.
    pub next_scope: u64,
}

#[cfg(feature = "boundary-counters")]
impl State {
    /// The counts of the innermost open scope, if one is open.
    fn innermost(&mut self) -> Option<&mut BoundaryCounters> {
        self.scopes.last_mut().map(|scope| &mut scope.counts)
    }

    /// Count `calls` calls.
    pub fn add_calls(&mut self, calls: u64) {
        self.counters.calls += calls;
        if let Some(counts) = self.innermost() {
            counts.calls += calls;
        }
    }

    /// Count `bytes` bytes.
    pub fn add_bytes(&mut self, bytes: u64) {
        self.counters.bytes += bytes;
        if let Some(counts) = self.innermost() {
            counts.bytes += bytes;
        }
    }

    /// Count `ms` milliseconds of boundary time.
    pub fn add_boundary_ms(&mut self, ms: f64) {
        self.counters.boundary_ms += ms;
        if let Some(counts) = self.innermost() {
            counts.boundary_ms += ms;
        }
    }

    /// Count `ms` milliseconds of host time.
    pub fn add_host_ms(&mut self, ms: f64) {
        self.counters.host_ms += ms;
        if let Some(counts) = self.innermost() {
            counts.host_ms += ms;
        }
    }

    /// Count the time each running clock ran until `now` into the
    /// innermost open scope, and run each on from `now`, so that a
    /// scope that opens or closes next counts only its own time.
    pub fn settle(&mut self, now: f64) {
        if let Some(since) = self.running_since {
            self.add_boundary_ms(now - since);
            self.running_since = Some(now);
        }
        if let Some(since) = self.host_since {
            self.add_host_ms(now - since);
            self.host_since = Some(now);
        }
    }

    /// Whether a clock runs, so that a scope that opens or closes has
    /// time to settle.
    pub fn timing(&self) -> bool {
        self.running_since.is_some() || self.host_since.is_some()
    }
}

#[cfg(feature = "boundary-counters")]
thread_local! {
    /// This thread's counters and clock.
    pub static STATE: RefCell<State> = RefCell::new(State::default());
}
