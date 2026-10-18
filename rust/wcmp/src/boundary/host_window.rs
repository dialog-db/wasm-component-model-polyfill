// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use super::clock::now_ms;
#[cfg(feature = "boundary-counters")]
use super::state::STATE;

/// A stretch of host code: a host function, a poll of a host future, or
/// a host destructor. The boundary clock stops while the guard lives,
/// and runs again when it drops, if it ran when the window opened.
pub struct HostWindow {
    /// Whether the window stopped the clock, and so starts it again.
    #[cfg(feature = "boundary-counters")]
    paused: bool,
}

impl HostWindow {
    /// Stop the boundary clock, if it runs, until the window closes.
    #[inline]
    pub fn enter() -> Self {
        #[cfg(feature = "boundary-counters")]
        {
            let paused = STATE.with(|state| {
                let mut state = state.borrow_mut();
                match state.running_since.take() {
                    Some(since) => {
                        state.counters.boundary_ms += now_ms() - since;
                        true
                    }
                    None => false,
                }
            });
            HostWindow { paused }
        }
        #[cfg(not(feature = "boundary-counters"))]
        HostWindow {}
    }
}

#[cfg(feature = "boundary-counters")]
impl Drop for HostWindow {
    fn drop(&mut self) {
        if self.paused {
            STATE.with(|state| state.borrow_mut().running_since = Some(now_ms()));
        }
    }
}
