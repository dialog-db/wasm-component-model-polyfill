// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the scheduler has to do before a turn goes on, under a
//! provider that resumes a thread only where the store runs no guest
//! code.

use super::in_flight::InFlight;
use super::instance_id::InstanceId;
use super::plan::Plan;
use super::subtask_id::SubtaskId;

/// What the scheduler has to do before anything else, under a
/// provider that runs a thread once the driver of the store awaits it
/// rather than inside the call that asks for it, which is the
/// host-suspension provider.
///
/// A resume is made only where the store runs no guest code, which is
/// a turn of a driver, and the thread runs once the turn returned
/// control to the driver, which awaits it. A frame that has to resume a thread
/// elsewhere leaves the resumption to the store, as the switch the
/// thread would make: the thread is named to run next, and the frame
/// goes no further. A frame that cannot wait for it suspends the
/// thread it runs in with a plan. Everything here is empty under
/// every other provider, and with none.
///
/// A turn that finds any of it runs it first and nothing else, in
/// this order: the thread it resumed, the plans that stops left, the
/// start a frame left to the store, the thread named to run next, and
/// the innermost plan. It returns control to the driver while a thread
/// runs, and takes up where it stopped once the thread stopped.
pub struct DeferredWork<T: 'static> {
    /// The thread a turn resumed or started, which the driver runs, and
    /// whose stop the scheduler waits for. Frames that run while it is set
    /// run inside that thread, so it is no work they left.
    pub resumed: Option<InFlight<T>>,
    /// Whether the resume of that thread was made and the frame that
    /// made it has yet to return control to the host executor. That
    /// frame goes no further: the thread runs once control is back
    /// with the executor, and the frames it runs are its own.
    pub resume_issued: bool,
    /// The thread whose start a frame inside a guest call left to the
    /// store, which the driver runs once the thread the frame runs in
    /// has suspended for it. It holds the thread's scopes until then.
    /// The frame that started it goes no further until it stops.
    pub deferred_start: Option<InFlight<T>>,
    /// Whether the thread named to run next is a resumption a frame
    /// could not make where it stood.
    pub next_left: bool,
    /// Plans whose threads suspended since the scheduler last ran,
    /// the innermost first: a thread a trampoline began stops before
    /// the thread the trampoline runs in.
    pub stopped: Vec<Plan<T>>,
    /// The plans the scheduler runs, the innermost last, each with its
    /// owner's scopes on the stack of current scopes.
    pub plans: Vec<Plan<T>>,
    /// The plan a trampoline left for the thread it runs in, which the
    /// thread's shim is about to suspend for.
    pub request: Option<Plan<T>>,
    /// Whether the item that stopped last for the work still owes the
    /// evaluation of the waiting threads' conditions.
    pub note_owed: bool,
    /// Whether the first part of the blocking built-in that runs now may
    /// leave the start of a thread to the store, which the next start of
    /// a thread takes: the built-in's thread suspends for the start
    /// then, and leaves the rest of the built-in as a plan.
    pub may_defer_start: bool,
    /// The subtask of a nested-start mark a trampoline put on the
    /// stack, when the mark comes off once the work is done rather than
    /// as the trampoline returns.
    pub ends_nested_start: Option<SubtaskId>,
    /// The instance of a nested start that left work to the store, with
    /// the may-not-suspend flag it had before the start cleared it. The
    /// flag stays clear until the nested-start mark comes off, as it
    /// would until the started thread first stopped.
    pub nested_may_not_suspend: Option<(InstanceId, bool)>,
    /// Whether a thread-switch mark comes off then.
    pub ends_thread_switch: bool,
    /// Whether the nested turn that stopped last for the work stopped
    /// in the item that ends it, so that nothing of it is left to go
    /// on with. Its caller takes this at once.
    pub stopped_at_end: bool,
    /// Whether the driver's turn stopped for the work, and goes on
    /// where it stopped once the work is done.
    pub turn_open: bool,
    /// Whether the item the driver's turn stopped in owes the
    /// evaluation of the waiting threads' conditions.
    pub turn_note_owed: bool,
}

impl<T: 'static> DeferredWork<T> {
    /// Whether anything is left to do before the frame that runs now
    /// may go on: a resume it made, the failure of a start it made, a
    /// resumption left to the store, or a plan a stop left.
    pub fn pending(&self) -> bool {
        self.resume_issued
            || self.deferred_start.is_some()
            || self.next_left
            || !self.stopped.is_empty()
    }

    /// Whether the store is inside work of this kind: a thread a turn
    /// resumed, a turn that stopped for it, a plan it runs, or
    /// anything still pending.
    pub fn busy(&self) -> bool {
        self.pending() || self.resumed.is_some() || !self.plans.is_empty() || self.turn_open
    }

    /// The slot of the thread a turn resumed, or, with `deferred_start`,
    /// of the thread whose start a frame left to the store.
    pub fn in_flight(&mut self, deferred_start: bool) -> &mut Option<InFlight<T>> {
        if deferred_start {
            &mut self.deferred_start
        } else {
            &mut self.resumed
        }
    }
}

impl<T: 'static> Default for DeferredWork<T> {
    fn default() -> Self {
        Self {
            resumed: None,
            resume_issued: false,
            deferred_start: None,
            next_left: false,
            stopped: Vec::new(),
            plans: Vec::new(),
            request: None,
            note_owed: false,
            may_defer_start: false,
            ends_nested_start: None,
            nested_may_not_suspend: None,
            ends_thread_switch: false,
            stopped_at_end: false,
            turn_open: false,
            turn_note_owed: false,
        }
    }
}
