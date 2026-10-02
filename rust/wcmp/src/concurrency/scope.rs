// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One entry of the store's stack of current scopes.

use super::lower_kind::LowerKind;
use super::subtask_id::SubtaskId;
use super::task_id::TaskId;
use super::thread_id::ThreadId;

/// One entry of the store's stack of current scopes.
///
/// The top of the stack is the current scope. A host call into an
/// export pushes the export's task; a guest call into a host
/// function pushes the subtask of that call, which stays on the
/// stack while the host side runs; an adapter's enter intrinsic
/// pushes the callee's task and its exit intrinsic pops it. The
/// stack is a stack because synchronous calls nest on the one real
/// stack.
///
/// A start intrinsic that runs an `async`-typed callee from inside
/// its own frame pushes a [`NestedStart`](Self::NestedStart) mark
/// under the callee's task and takes it off when the callee returns
/// to it. The mark is not a scope: nothing counts a borrow or a lend
/// against it, and the current scope is the innermost entry that is
/// a task or a subtask. It records where the real stack holds a
/// frame that a stack switch would return to, which is what the
/// cause of a failed block reads.
///
/// A thread built-in that switches to a thread that has never run
/// starts it the same way, from inside its own frame, and marks that
/// with a [`ThreadSwitch`](Self::ThreadSwitch) mark for as long as
/// the started thread runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// A call into an export.
    Task(TaskId),
    /// A call out through an import.
    Subtask(SubtaskId),
    /// A thread a trampoline started from inside itself, on the real
    /// stack above the trampoline's frame. The reference runs that
    /// thread on a stack of its own and returns to the trampoline
    /// when it suspends. Without a stack switch, everything below
    /// the mark stays where it is until the thread returns.
    ///
    /// What the caller would do once control came back depends on
    /// the call. After an asynchronous lower the caller's own code
    /// goes on. After a synchronous lower the caller waits for the
    /// callee's result, and its own code goes on only once the
    /// callee has resolved.
    NestedStart {
        /// The caller's record of the call.
        subtask: SubtaskId,
        /// How the caller lowered the call.
        lower: LowerKind,
    },
    /// A thread a switching thread built-in started from inside
    /// itself, on the real stack above the built-in's frame. The
    /// reference runs the started thread on a stack of its own, and
    /// the frame that resumed the switching thread runs it next.
    /// Without a stack switch, the switching thread stays below the
    /// mark until the started thread returns.
    ///
    /// The switching thread would go on once control came back to it
    /// when it is not suspended: it yielded to the started thread, or
    /// a `thread.resume-later` has made it ready again since.
    ThreadSwitch {
        /// The thread whose built-in switched.
        thread: ThreadId,
    },
}

impl Scope {
    /// Whether the entry is a mark of a thread started from inside a
    /// frame, rather than a scope. Nothing counts a borrow or a lend
    /// against a mark.
    pub fn is_mark(self) -> bool {
        matches!(self, Self::NestedStart { .. } | Self::ThreadSwitch { .. })
    }
}
