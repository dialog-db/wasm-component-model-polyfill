// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One call out through an import.

use crate::resource::TableId;

use super::call_bridge::CallBridge;
use super::subtask_state::SubtaskState;
use super::task_id::TaskId;
use super::waitable_state::WaitableState;

/// The record of one call out through an import.
///
/// A subtask is the scope a borrow lifted out of the caller's owning
/// handle is lent to, whether the callee is a host function or
/// another component's export: the lift raises the lend count on the
/// owning entry and records the entry here, and delivering the
/// subtask's resolution lowers those counts again. A resolution is
/// delivered when the caller's thread receives the subtask event, or
/// when a synchronous lower returns. `HandleTables::lend_to` states
/// the rule.
///
/// A subtask is also a waitable, so the record carries the waitable
/// state a guest waits on: the pending event slot, the set the
/// subtask joined, and the synchronous-waiter flag.
pub struct Subtask {
    /// How far the call has got.
    pub state: SubtaskState,
    /// The owning handle-table entries the caller lent for the call,
    /// as `(table, index)`. The reference names this list `lenders`.
    /// Emptied when the resolution is delivered.
    pub lenders: Vec<(TableId, u32)>,
    /// Whether the subtask's resolution has been delivered, which is
    /// what released the handles in `lenders`. The reference says
    /// the same thing by emptying its own list. Dropping a subtask
    /// whose resolution was not delivered traps.
    pub resolve_delivered: bool,
    /// The waitable state: what a thread waiting on this subtask
    /// consults.
    pub waitable: WaitableState,
    /// Whether the caller asked for the call to be cancelled, with
    /// `subtask.cancel`. A second request traps.
    pub cancel_requested: bool,
    /// The subtask's index in the caller instance's handle table,
    /// while the caller holds an entry for it. A call that resolves
    /// before the lower returns is never given one, and
    /// `subtask.drop` takes the entry away again.
    ///
    /// The index is the first payload of every subtask event, and
    /// its presence is what says a starting subtask has a caller to
    /// notify: a callee the gate held is started later, when the
    /// caller already holds the entry the lower returned.
    pub handle: Option<u32>,
    /// The callee's task, for a call into another component's
    /// export. `None` for a call into a host function, which has no
    /// task of its own.
    pub callee: Option<TaskId>,
    /// The two functions the fused adapter generated for the call,
    /// for a call the prepare intrinsic set up. `None` for a call
    /// into a host function, where the polyfill lifts and lowers the
    /// values itself.
    pub bridge: Option<CallBridge>,
}

impl Subtask {
    /// Construct a subtask in its starting state: the call was made
    /// and its parameters have not been lifted yet.
    pub fn new() -> Self {
        Self {
            state: SubtaskState::Starting,
            lenders: Vec::new(),
            resolve_delivered: false,
            waitable: WaitableState::new(),
            cancel_requested: false,
            handle: None,
            callee: None,
            bridge: None,
        }
    }
}

impl Default for Subtask {
    fn default() -> Self {
        Self::new()
    }
}
