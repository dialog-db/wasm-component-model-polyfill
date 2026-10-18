// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use std::collections::HashMap;
#[cfg(feature = "boundary-counters")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "boundary-counters")]
use super::{BoundaryCounters, StoreBoundaryCounters};

/// The boundary counters of one store, which the store holds and each
/// scope that counts for the store adds to as it closes. Without the
/// `boundary-counters` feature it holds nothing.
///
/// The counts sit behind a lock rather than in the thread's state, so a
/// store that moves to another thread natively keeps its counts. A
/// scope takes the lock once, as it closes.
#[derive(Clone, Default)]
pub struct BoundarySink {
    #[cfg(feature = "boundary-counters")]
    tally: Arc<Mutex<Tally>>,
}

/// What a store has counted, keyed by import as the scopes add to it.
#[cfg(feature = "boundary-counters")]
#[derive(Default)]
struct Tally {
    total: BoundaryCounters,
    imports: HashMap<Arc<str>, BoundaryCounters>,
    other: BoundaryCounters,
}

impl BoundarySink {
    /// Add `counts`, which a scope counted for `import`, or for the rest
    /// of the store's work when `import` is `None`.
    #[cfg(feature = "boundary-counters")]
    pub fn add(&self, import: Option<&Arc<str>>, counts: &BoundaryCounters) {
        let Ok(mut tally) = self.tally.lock() else {
            return;
        };
        add(&mut tally.total, counts);
        match import {
            Some(import) => add(tally.imports.entry(Arc::clone(import)).or_default(), counts),
            None => add(&mut tally.other, counts),
        }
    }

    /// What the store has counted.
    #[cfg(feature = "boundary-counters")]
    pub fn read(&self) -> StoreBoundaryCounters {
        self.tally
            .lock()
            .map(|tally| tally.counters())
            .unwrap_or_default()
    }

    /// What the store has counted, and reset its counts to zero.
    #[cfg(feature = "boundary-counters")]
    pub fn take(&self) -> StoreBoundaryCounters {
        self.tally
            .lock()
            .map(|mut tally| std::mem::take(&mut *tally).counters())
            .unwrap_or_default()
    }
}

#[cfg(feature = "boundary-counters")]
impl Tally {
    /// The counts as the store answers them.
    fn counters(&self) -> StoreBoundaryCounters {
        StoreBoundaryCounters {
            total: self.total,
            imports: self
                .imports
                .iter()
                .map(|(import, counts)| (import.to_string(), *counts))
                .collect(),
            other: self.other,
        }
    }
}

/// Add `counts` to `into`.
#[cfg(feature = "boundary-counters")]
fn add(into: &mut BoundaryCounters, counts: &BoundaryCounters) {
    into.calls += counts.calls;
    into.bytes += counts.bytes;
    into.boundary_ms += counts.boundary_ms;
    into.host_ms += counts.host_ms;
}
