// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use super::clock::now_ms;
#[cfg(feature = "boundary-counters")]
use super::state::STATE;

/// A stretch of the polyfill's own work at the boundary, whose time
/// the boundary counters add up while the guard lives.
///
/// Segments nest: one that starts while another runs adds nothing of
/// its own, so a stretch is counted once. One that starts inside a
/// [`HostWindow`](super::HostWindow), where the host has called back
/// into the polyfill, runs the clock until it ends, and stops the host
/// clock meanwhile.
pub struct BoundarySegment {
    /// Whether this segment started the clock, and so stops it.
    #[cfg(feature = "boundary-counters")]
    started: bool,
    /// Whether this segment stopped the host clock, and so starts it again.
    #[cfg(feature = "boundary-counters")]
    paused_host: bool,
}

impl BoundarySegment {
    /// Start a segment, unless one runs.
    #[inline]
    pub fn enter() -> Self {
        #[cfg(feature = "boundary-counters")]
        {
            let (started, paused_host) = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if state.running_since.is_some() {
                    return (false, false);
                }
                let now = now_ms();
                state.running_since = Some(now);
                let paused_host = match state.host_since.take() {
                    Some(since) => {
                        state.add_host_ms(now - since);
                        true
                    }
                    None => false,
                };
                (true, paused_host)
            });
            BoundarySegment {
                started,
                paused_host,
            }
        }
        #[cfg(not(feature = "boundary-counters"))]
        BoundarySegment {}
    }
}

#[cfg(feature = "boundary-counters")]
impl Drop for BoundarySegment {
    fn drop(&mut self) {
        if !self.started {
            return;
        }
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let now = now_ms();
            if let Some(since) = state.running_since.take() {
                state.add_boundary_ms(now - since);
            }
            if self.paused_host {
                state.host_since = Some(now);
            }
        });
    }
}
