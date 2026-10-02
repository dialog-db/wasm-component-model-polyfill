// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The waitable state one waitable record carries.

use super::event::Event;
use super::waitable_set_id::WaitableSetId;

/// The state every waitable record carries, whatever kind of
/// waitable the record is.
///
/// A subtask record holds one of these, and so will each stream and
/// future end once those features land. Keeping the three fields
/// together is what lets one set of store operations serve every
/// waitable kind; the reference reaches the same place by making
/// every waitable kind a subclass of its `Waitable`.
pub struct WaitableState {
    /// The one event the waitable has pending for the thread that
    /// waits on it. The scheduler records readiness by filling this
    /// slot, and delivery empties it.
    pub pending_event: Option<Event>,
    /// The waitable set the waitable joined, or `None` when it has
    /// joined none.
    pub set: Option<WaitableSetId>,
    /// Whether a thread is waiting on this waitable on its own,
    /// outside any set. A waitable with such a waiter cannot join a
    /// set, and a waitable in a set cannot take such a waiter.
    pub synchronous_waiter: bool,
}

impl WaitableState {
    /// Construct the state of a fresh waitable: no pending event, no
    /// set, and no synchronous waiter.
    pub fn new() -> Self {
        Self {
            pending_event: None,
            set: None,
            synchronous_waiter: false,
        }
    }
}

impl Default for WaitableState {
    fn default() -> Self {
        Self::new()
    }
}
