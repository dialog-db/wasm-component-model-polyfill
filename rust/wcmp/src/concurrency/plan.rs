// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Work a trampoline left for the scheduler before its thread goes
//! on.

use super::instance_id::InstanceId;
use super::readiness::Readiness;
use super::seam_wait::SeamWait;
use super::thread_id::ThreadId;

/// Work a trampoline left for the scheduler, under a provider that
/// resumes a thread only where the store runs no guest code, before
/// the thread the trampoline runs in goes on.
///
/// The reference sometimes resumes a suspended thread from inside a
/// trampoline and continues in the trampoline once that thread stops:
/// a synchronous call of a sync-typed function runs the ready threads
/// of its own instance until its task resolves, and a thread a nested
/// start began can switch to a suspended thread before it first
/// suspends. The host-suspension provider cannot resume a suspended stack from
/// inside a synchronous frame. The trampoline therefore leaves what
/// it has yet to do as a plan, and its shim suspends the thread it
/// runs in as well. The scheduler takes the plan up where the store
/// runs no guest code: it runs the resumption the trampoline could
/// not make, and whatever that thread switches to, then the rest of
/// the plan's wait, and nothing else. It runs no other item and polls
/// no host task but those the wait itself would have. Then it resumes
/// the trampoline's thread, whose shim tries the built-in again and
/// reads the plan's outcome. No other guest code runs in the interval,
/// so a guest cannot tell.
///
/// Plans nest. A thread the scheduler resumes for one plan can leave
/// a plan of its own, and the inner plan runs to its end, resuming its
/// thread, before the outer plan goes on.
pub struct Plan<T: 'static> {
    /// The thread whose stack suspended for the plan, which the plan
    /// resumes once it is done. Known once the stack has suspended.
    pub owner: Option<ThreadId>,
    /// The thread whose blocking built-in the plan serves: the owner,
    /// or the callee of a synchronous call that runs on the owner's
    /// stack.
    pub blocked: ThreadId,
    /// Where the owner's scopes begin on the stack of current scopes
    /// while the plan runs.
    pub base: usize,
    /// The rest of the built-in's wait, which runs once the work the
    /// trampoline left is done. `None` for a trampoline that waits on
    /// nothing but that work.
    pub wait: Option<SeamWait<T>>,
    /// The condition of the wait that begins once the work the
    /// trampoline left is done, for a built-in whose first part left
    /// it: the wait begins then, as it would have begun once the first
    /// part returned.
    pub then_wait: Option<Readiness>,
    /// Whether the built-in suspends its thread once the work is done,
    /// as the stack-switching form would have: its condition is
    /// recorded on the thread then, and the thread resumes at once only
    /// when the condition holds, and otherwise waits as any suspended
    /// thread does.
    pub suspends: bool,
    /// Whether the nested-start mark the trampoline put on the stack
    /// comes off once the work the trampoline left is done.
    pub ends_nested_start: bool,
    /// The instance whose may-not-suspend flag goes back to the value
    /// here once the nested-start mark comes off.
    pub restores_may_not_suspend: Option<(InstanceId, bool)>,
    /// Whether the thread-switch mark the trampoline put on the stack
    /// comes off then.
    pub ends_thread_switch: bool,
    /// Whether the item that stopped for the work still owes the
    /// evaluation of the waiting threads' conditions that follows
    /// every item.
    pub note_owed: bool,
}

impl<T: 'static> Plan<T> {
    /// A plan for the built-in `blocked` waits in, with nothing to do
    /// yet but the work the trampoline left.
    pub fn new(blocked: ThreadId) -> Self {
        Self {
            owner: None,
            blocked,
            base: 0,
            wait: None,
            then_wait: None,
            suspends: false,
            ends_nested_start: false,
            restores_may_not_suspend: None,
            ends_thread_switch: false,
            note_owed: false,
        }
    }
}
