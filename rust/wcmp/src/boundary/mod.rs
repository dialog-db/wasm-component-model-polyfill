// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The boundary counters: how many calls cross from a guest into the
//! host, how many bytes the polyfill copies across the boundary in
//! either direction, and how long the polyfill spends lifting,
//! lowering, and dispatching, without the time inside the host.
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
