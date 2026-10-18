// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use super::state::STATE;

/// What the boundary counters counted on this thread since they were
/// last taken.
///
/// A call is a guest's call to a lowered host import, or its drop of a
/// handle to a host resource. The bytes are those the polyfill reads
/// from and writes to guest memory for a crossing between a guest and
/// the host, in either direction, including stream and future copies
/// with a host end; a move between two guests is not counted. The
/// boundary time is the time the polyfill spends lifting, lowering, and
/// dispatching those crossings, without the time inside host functions
/// and host futures, which is the host time.
///
/// These counters are the thread's: they add up the work of every store
/// the thread runs. [`Store::boundary_counters`] gives one store's own,
/// with a breakdown by host import. They exist only with the
/// `boundary-counters` feature.
///
/// The times come from a clock in milliseconds: a monotonic clock
/// natively, and `performance.now()` in a page or a worker, whose
/// resolution the browser coarsens. The High Resolution Time
/// specification sets it at 100 microseconds or coarser, and at
/// 5 microseconds or coarser in a cross-origin-isolated context. Chrome
/// gives exactly those two since Chrome 91. Firefox and Safari were
/// reported in 2020 to give 1 millisecond, and Firefox 20 microseconds
/// when cross-origin isolated. A boundary crossing often takes less
/// than one tick, so a count of a few calls can read as no time, or as
/// a whole tick. Add the times over many calls, as over a frame, before
/// reading them as a cost per call.
///
/// [`Store::boundary_counters`]: crate::Store::boundary_counters
#[cfg(feature = "boundary-counters")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundaryCounters {
    /// The calls from a guest into the host.
    pub calls: u64,
    /// The bytes copied across the boundary, in both directions.
    pub bytes: u64,
    /// The time the polyfill spent at the boundary, in milliseconds.
    pub boundary_ms: f64,
    /// The time inside host functions, host futures, and host destructors
    /// that guests' crossings reached, in milliseconds, without the time
    /// of the polyfill's own work they called back into.
    pub host_ms: f64,
}

#[cfg(feature = "boundary-counters")]
impl BoundaryCounters {
    /// What the counters hold now.
    pub fn read() -> Self {
        STATE.with(|state| state.borrow().counters)
    }

    /// What the counters hold now, and reset them to zero. A host that
    /// takes them once per frame gets the counts of each frame.
    pub fn take() -> Self {
        STATE.with(|state| std::mem::take(&mut state.borrow_mut().counters))
    }
}

/// Count one call from a guest into the host.
#[inline]
pub fn count_call() {
    #[cfg(feature = "boundary-counters")]
    STATE.with(|state| state.borrow_mut().add_calls(1));
}

/// Count `bytes` bytes copied across the boundary.
#[inline]
pub fn count_bytes(bytes: usize) {
    #[cfg(feature = "boundary-counters")]
    STATE.with(|state| state.borrow_mut().add_bytes(bytes as u64));
    #[cfg(not(feature = "boundary-counters"))]
    let _ = bytes;
}
