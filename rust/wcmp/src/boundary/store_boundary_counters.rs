// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::collections::BTreeMap;

#[cfg(feature = "boundary-counters")]
use super::BoundaryCounters;

/// What the boundary counters counted for one store since they were
/// last taken, with a breakdown by host import.
///
/// [`Store::boundary_counters`] and [`Store::take_boundary_counters`]
/// answer it. The counts are the store's own: two stores on one thread,
/// or a store whose calls interleave with another's, count apart. A
/// count goes to the store whose work made it, and to the host import
/// whose call made it, when one did: the call itself, the bytes of its
/// arguments and its result, the polyfill's time lifting and lowering
/// them, and the time inside the host function or the polls of its
/// future. An import a host function calls back into a guest through
/// counts the inner call to the inner import alone.
///
/// The rest of the store's work counts under [`other`](Self::other): the
/// calls into the guest the host makes and their values, the drops of
/// host resources, and the copies of streams and futures with a host end.
/// The imports' counts and `other` add up to [`total`](Self::total).
///
/// The counts of a stretch of work reach the store when it ends, so a
/// read from inside a host function does not yet see the call it is in.
/// A host that reads between two frames sees every call of the frame.
///
/// Wasmtime has no counterpart. The type exists only with the
/// `boundary-counters` feature.
///
/// [`Store::boundary_counters`]: crate::Store::boundary_counters
/// [`Store::take_boundary_counters`]: crate::Store::take_boundary_counters
#[cfg(feature = "boundary-counters")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoreBoundaryCounters {
    /// Everything the store counted.
    pub total: BoundaryCounters,
    /// The counts of each host import the store's guests called, by the
    /// import's name: the component's import name, and the names inside
    /// it down to the function, joined by `#`, as in
    /// `wasi:webgpu/webgpu@0.3.0-rc.2#[method]gpu-queue.submit`.
    pub imports: BTreeMap<String, BoundaryCounters>,
    /// What the store counted outside any host import's call.
    pub other: BoundaryCounters,
}
