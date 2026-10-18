// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::sync::Arc;

#[cfg(feature = "boundary-counters")]
use super::BoundaryCounters;
#[cfg(feature = "boundary-counters")]
use super::boundary_sink::BoundarySink;

/// One open [`BoundaryScope`](super::BoundaryScope), as the thread's
/// state keeps it: what it has counted so far, and where that goes when
/// it closes.
#[cfg(feature = "boundary-counters")]
pub struct OpenScope {
    /// The scope's identity, which its guard closes it by.
    pub id: u64,
    /// The counters of the store the scope counts for.
    pub sink: BoundarySink,
    /// The host import the scope counts for, or `None` for the rest of
    /// the store's work.
    pub import: Option<Arc<str>>,
    /// What the scope has counted so far.
    pub counts: BoundaryCounters,
}
