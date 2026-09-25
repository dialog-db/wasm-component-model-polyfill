//! The contract a provider of the scheduler's suspend capability
//! meets.

use wasm_runtime_layer::{Func as RuntimeFunc, Val as RuntimeVal};

use crate::error::Result;
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
/// A resumption returns when the thread suspends again or finishes.
/// The stack-switching provider does both at once, inside the call.
///
/// The provider is shared and never taken out of the store while a
/// thread is suspended, so both methods take `&self`. A caller that
/// reaches the provider through the store clones its handle first,
/// and hands the store in beside it.
pub trait SuspendProvider<T: 'static>: 'static {
    /// Start `entry` with `args` as `thread`, on a stack of its own,
    /// and run it until it finishes or first suspends.
    ///
    /// The caller is the scheduler or a trampoline. From a
    /// trampoline this is a nested start: the new thread runs above
    /// the trampoline's frame, and control comes back to the
    /// trampoline when the thread suspends or finishes.
    ///
    /// It answers the entry's results when the entry finished, and
    /// [`EntryStatus::Suspended`] when it suspended, in which case
    /// the provider keeps the thread until a [`resume`](Self::resume)
    /// names it. It fails with the entry's trap, or when the provider
    /// has no wrapper for the entry's type.
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus>;

    /// Resume the suspended `thread`, and run it until it finishes or
    /// suspends again. It answers as [`start`](Self::start) does, and
    /// fails when `thread` is not suspended.
    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus>;
}
