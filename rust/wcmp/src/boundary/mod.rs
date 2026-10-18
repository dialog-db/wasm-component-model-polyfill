// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The boundary counters: how many calls cross from a guest into the
//! host, how many bytes the polyfill copies across the boundary in
//! either direction, and how long the polyfill spends lifting,
//! lowering, and dispatching, without the time inside the host, and how
//! long the host functions take.
//!
//! They count only with the `boundary-counters` feature, which is off by
//! default. Without it, every function here does nothing, and each guard
//! is empty, so the call sites carry no `cfg`.

mod clock;
mod counters;
mod host_window;
mod segment;
mod state;

#[cfg(feature = "boundary-counters")]
pub use counters::BoundaryCounters;
pub use counters::{count_bytes, count_call};
pub use host_window::HostWindow;
pub use segment::BoundarySegment;

#[cfg(all(test, feature = "boundary-counters"))]
mod tests {
    use super::clock::now_ms;
    use super::{BoundaryCounters, BoundarySegment, HostWindow};

    /// Spend at least `ms` milliseconds on the counters' own clock, and give
    /// how long it took: under load, a thread can be preempted mid-spin.
    fn spend(ms: f64) -> f64 {
        let start = now_ms();
        let until = start + ms;
        while now_ms() < until {
            std::hint::spin_loop();
        }
        now_ms() - start
    }

    #[wcmp_macros::test]
    fn it_keeps_the_host_time_apart_from_the_boundary_time_it_calls_back_into() {
        BoundaryCounters::take();
        let (mut boundary, mut host) = (0.0, 0.0);
        {
            let _call = BoundarySegment::enter();
            boundary += spend(2.0);
            {
                let _host = HostWindow::enter();
                host += spend(4.0);
                {
                    // The host calls back into the polyfill.
                    let _callback = BoundarySegment::enter();
                    boundary += spend(8.0);
                }
                host += spend(4.0);
            }
            boundary += spend(2.0);
        }
        let counted = BoundaryCounters::take();
        // The guards' own work, between the spins, is a few microseconds,
        // unless the thread is preempted there.
        assert!(
            (counted.boundary_ms - boundary).abs() < 3.0,
            "{counted:?} against {boundary}"
        );
        assert!(
            (counted.host_ms - host).abs() < 3.0,
            "{counted:?} against {host}"
        );
    }
}
