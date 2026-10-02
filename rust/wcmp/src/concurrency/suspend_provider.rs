// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The contract a provider of the scheduler's suspend capability
//! meets.

use core::task::{Poll, Waker};

use crate::error::Result;
use crate::runtime_layer::{Func as RuntimeFunc, FuncType, Val as RuntimeVal};
use crate::store::StoreContext;

use super::entry_status::EntryStatus;
use super::thread_id::ThreadId;

/// The contract a provider of the scheduler's suspend capability
/// meets.
///
/// The reference lets a running guest thread block inside a
/// built-in, and Wasmtime serves that by suspending the fiber the
/// guest runs on. The polyfill runs a guest on the one real stack,
/// so it needs a mechanism that sets a guest stack aside and resumes
/// it later: a provider. A provider must do six things:
///
/// 1. Start a thread entry on a stack of its own, from the scheduler
///    or from inside a trampoline. A thread entry is a guest function
///    that starts a thread: a task's core function, a callback, or a
///    thread's start function. That is [`start`](Self::start).
/// 2. Suspend the running thread when a blocking built-in is not
///    ready, with only WebAssembly frames between the start of that
///    stack and the point of suspension.
/// 3. Resume a suspended thread later, with the result of the
///    built-in it suspended in. That is [`resume`](Self::resume).
/// 4. Keep any number of threads suspended at once, and resume them
///    in any order.
/// 5. Report to the caller whether a thread entry finished or
///    suspended, at the moment it does so, which is the
///    [`EntryStatus`] both methods answer.
/// 6. Drop a suspended thread without resuming it when its store
///    drops. No destructor runs, as for anything else the store
///    holds.
///
/// A mechanism that cannot meet all six is not a provider, and the
/// polyfill never fills the capability with less. Where no provider
/// exists, a blocking built-in runs the waiting work in a nested turn
/// above the blocked call instead.
///
/// The second duty has no method, because no host frame can perform
/// it: a host function is not WebAssembly, so a suspension from
/// inside one would put a frame that is not WebAssembly on the
/// thread's stack. Both providers suspend in WebAssembly instead, in
/// the switch module, a small core module the polyfill generates for
/// each store. A guest imports the switch module's shim for each
/// blocking built-in in place of the host trampoline. The shim calls
/// a host function that tries the built-in and returns at once. It
/// returns the built-in's result when it is ready, and otherwise
/// suspends in the provider's own form of suspension and tries again
/// once the thread resumes. The try and the finish of the built-in
/// are host frames that return before the shim suspends, so they are
/// not on the stack at the point of suspension. That is also how the
/// third duty hands the thread the built-in's result: the shim's
/// retry computes it after the resume.
///
/// The switch module also wraps each thread entry. The wrapper calls
/// the entry and hands its results to a host function before it
/// returns, so the provider has them the moment the entry finishes,
/// whether or not it suspended on the way.
///
/// A resumption can complete at once or later. The stack-switching
/// provider resumes a thread inside the call that asks for it, which
/// returns when the thread suspends again or finishes. The
/// host-suspension provider resumes a thread once the driver of the
/// store awaits it, never inside that call, so its resume answers [`EntryStatus::Running`], and the caller
/// learns where the thread stopped from
/// [`poll_stop`](Self::poll_stop). The scheduler treats both the same
/// way: a turn that resumes a thread waits until the thread stops,
/// and runs nothing else in between.
///
/// The provider is shared and never taken out of the store while a
/// thread is suspended, so every method takes `&self`. A caller that
/// reaches the provider through the store clones its handle first,
/// and hands the store in beside it.
pub trait SuspendProvider<T: 'static>: 'static {
    /// Start `entry`, whose core type is `ty`, with `args` as `thread`,
    /// on a stack of its own, and run it until it finishes or first
    /// suspends. The caller names the type because a function reference
    /// the host received does not always carry one.
    ///
    /// The caller is the scheduler or a trampoline. From a
    /// trampoline this is a nested start: the new thread runs above
    /// the trampoline's frame, and control comes back to the
    /// trampoline when the thread suspends or finishes.
    ///
    /// It answers the entry's results when the entry finished, and
    /// [`EntryStatus::Suspended`] when it suspended, in which case
    /// the provider keeps the thread until a [`resume`](Self::resume)
    /// names it. A provider that learns of a failure only later
    /// answers [`EntryStatus::Running`] for an entry that failed, and
    /// [`poll_stop`](Self::poll_stop) then fails with the entry's
    /// trap. Otherwise it fails with the entry's trap itself, or when
    /// the provider has no wrapper for the entry's type.
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus>;

    /// Resume the suspended `thread`. A provider that runs it inside
    /// the call runs it until it finishes or suspends again, and
    /// answers as [`start`](Self::start) does. One that runs it later
    /// answers [`EntryStatus::Running`]. It fails when `thread` is not
    /// suspended.
    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus>;

    /// Where `thread` stopped, once a start or a resume of it answered
    /// [`EntryStatus::Running`]: finished or suspended again, or the
    /// trap it failed with. Pending until it stops, in which case
    /// `waker` is woken when it does.
    fn poll_stop(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        waker: &Waker,
    ) -> Poll<Result<EntryStatus>>;
}
