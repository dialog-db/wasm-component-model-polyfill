// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The work a poisoned store let go of, until it drops.

use super::host_reader::HostReader;
use super::host_task::HostTask;
use super::host_writer::HostWriter;
use super::item::Item;

/// The work a store let go of when a trap poisoned it, held from the
/// moment the scheduler gave it up to the moment its holder drops it.
///
/// The scheduler cannot drop this work itself. A host task's future,
/// a producer, and a consumer are host code, and dropping one runs
/// that code: its `Drop` can reach the store through the accessor it
/// holds, or lock something the store also locks. The scheduler is
/// reached with the store borrowed and, in places, with the handle
/// tables locked, so it hands the work out in one piece and the store
/// drops it where it holds no lock. An accessor reached from such a
/// `Drop` then fails with the cause a reach outside a poll, or inside
/// another reach, fails with, rather than deadlocking or lending the
/// store twice.
///
/// Dropping this is the whole of what happens to the work. No item
/// runs, no host task is polled, and no producer or consumer is
/// asked to finish: a poisoned store runs no more guest code, and
/// each of these would either run guest code or hand something to
/// guest code that will never take it.
pub struct DiscardedWork<T: 'static> {
    /// The guest work items: the switch slot, the ready queues, the
    /// resume-after-yield slot, the entry gate, and the held
    /// callbacks, in that order.
    pub items: Vec<Item<T>>,
    /// The host tasks: the store's own set, pending or woken, and the
    /// parked calls a poll completed but no lower took.
    pub host_tasks: Vec<HostTask<T>>,
    /// The producers of the writable ends the host serves.
    pub host_writers: Vec<Box<dyn HostWriter<T>>>,
    /// The consumers of the readable ends the host serves.
    pub host_readers: Vec<Box<dyn HostReader<T>>>,
}

impl<T: 'static> Drop for DiscardedWork<T> {
    /// Drop the work in a fixed order: the guest work items first,
    /// then the host tasks in the order the store held them, then the
    /// producers, then the consumers.
    fn drop(&mut self) {
        self.items.clear();
        self.host_tasks.clear();
        self.host_writers.clear();
        self.host_readers.clear();
    }
}
