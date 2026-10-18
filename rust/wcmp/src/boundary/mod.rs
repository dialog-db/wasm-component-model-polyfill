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

mod boundary_sink;
mod clock;
mod counters;
mod host_window;
mod import_name;
mod open_scope;
mod scope;
mod segment;
mod state;
mod store_boundary_counters;

pub use boundary_sink::BoundarySink;
#[cfg(feature = "boundary-counters")]
pub use counters::BoundaryCounters;
pub use counters::{count_bytes, count_call};
pub use host_window::HostWindow;
pub use import_name::ImportName;
pub use scope::BoundaryScope;
pub use segment::BoundarySegment;
#[cfg(feature = "boundary-counters")]
pub use store_boundary_counters::StoreBoundaryCounters;

#[cfg(all(test, feature = "boundary-counters"))]
mod tests {
    use super::clock::now_ms;
    use super::{
        BoundaryCounters, BoundaryScope, BoundarySegment, BoundarySink, HostWindow, ImportName,
    };

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

    #[wcmp_macros::test]
    fn it_counts_an_inner_imports_work_to_the_inner_import_alone() {
        let store = BoundarySink::default();
        let (outer, inner) = (
            ImportName::new(|| "outer".to_owned()),
            ImportName::new(|| "inner".to_owned()),
        );
        let (mut outer_host, mut inner_boundary) = (0.0, 0.0);
        {
            let _turn = BoundaryScope::enter(&store, &ImportName::default());
            let _call = BoundaryScope::enter(&store, &outer);
            let _segment = BoundarySegment::enter();
            super::count_call();
            super::count_bytes(8);
            {
                let _host = HostWindow::enter();
                outer_host += spend(4.0);
                {
                    // The host calls back into a guest, which calls
                    // another import.
                    let _callback = BoundaryScope::enter(&store, &inner);
                    let _segment = BoundarySegment::enter();
                    super::count_call();
                    super::count_bytes(16);
                    inner_boundary += spend(4.0);
                }
                outer_host += spend(4.0);
            }
        }
        let counted = store.take();
        let (outer, inner) = (counted.imports["outer"], counted.imports["inner"]);
        assert_eq!((outer.calls, outer.bytes), (1, 8), "{counted:?}");
        assert_eq!((inner.calls, inner.bytes), (1, 16), "{counted:?}");
        assert!(
            (outer.host_ms - outer_host).abs() < 3.0,
            "the outer import's host time leaves out the inner call: {counted:?}"
        );
        assert!(
            inner.host_ms < 3.0,
            "the inner import ran no host code: {counted:?}"
        );
        assert!(
            (inner.boundary_ms - inner_boundary).abs() < 3.0,
            "{counted:?} against {inner_boundary}"
        );
        assert_eq!(counted.other.calls, 0, "{counted:?}");
        assert_eq!(counted.total.calls, 2);
        assert_eq!(counted.total.bytes, 24);
    }
}
