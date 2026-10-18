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
/// and runs again when it drops, if it ran when the window opened. The
/// host clock runs while the guard lives, unless it already runs.
pub struct HostWindow {
    /// Whether the window stopped the clock, and so starts it again.
    #[cfg(feature = "boundary-counters")]
    paused: bool,
    /// Whether the window started the host clock, and so stops it.
    #[cfg(feature = "boundary-counters")]
    timing_host: bool,
}

impl HostWindow {
    /// Stop the boundary clock, if it runs, until the window closes.
    #[inline]
    pub fn enter() -> Self {
        #[cfg(feature = "boundary-counters")]
        {
            let (paused, timing_host) = STATE.with(|state| {
                let mut state = state.borrow_mut();
                let now = now_ms();
                let paused = match state.running_since.take() {
                    Some(since) => {
                        state.add_boundary_ms(now - since);
                        true
                    }
                    None => false,
                };
                let timing_host = state.host_since.is_none();
                if timing_host {
                    state.host_since = Some(now);
                }
                (paused, timing_host)
            });
            HostWindow {
                paused,
                timing_host,
            }
        }
        #[cfg(not(feature = "boundary-counters"))]
        HostWindow {}
    }
}

#[cfg(feature = "boundary-counters")]
impl Drop for HostWindow {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let now = now_ms();
            if self.timing_host
                && let Some(since) = state.host_since.take()
            {
                state.add_host_ms(now - since);
            }
            if self.paused {
                state.running_since = Some(now);
            }
        });
    }
}
