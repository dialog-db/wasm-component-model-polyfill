//! One guest execution.

use super::readiness::Readiness;
use super::task_id::TaskId;
use super::thread_id::ThreadId;
use super::thread_start::ThreadStart;
use super::waitable_set_id::WaitableSetId;

/// The record of one guest execution.
///
/// Every task has at least one thread, its implicit thread. The
/// thread carries the two context slots `context.get` and
/// `context.set` read and write: the slots moved here from one pair
/// per instantiation, because an adapter that saves them around a
/// callee must see the callee's own pair rather than one it shares
/// with the caller.
///
/// A task can hold more threads than its implicit one. Each further
/// thread is an explicit thread, which `thread.new-indirect` creates
/// suspended with the start function it will run. Every thread that
/// belongs to a component instance has an index in that instance's
/// thread table, which is what `thread.index` answers and what the
/// thread built-ins name a thread by.
pub struct Thread {
    /// The task that contains this thread.
    pub task: TaskId,
    /// The readiness condition the thread waits on, or `None` when
    /// it waits on nothing. The try part of a blocking built-in
    /// records it, and it stays here until the thread's wait ends:
    /// a thread parked on a waitable set names the set, and a
    /// thread inside a synchronous lower names the call's subtask.
    pub readiness: Option<Readiness>,
    /// The two context slots, which `context.get` reads and
    /// `context.set` writes.
    pub context: [i32; 2],
    /// The may-not-suspend flag of the callee instance as it was
    /// before this thread's task entered it, saved by the enter
    /// intrinsic and restored by the exit intrinsic. `None` when the
    /// task did not set the flag, which is the case for the task of
    /// an `async` callee. Wasmtime saves the same value in the same
    /// place.
    pub old_may_not_suspend: Option<bool>,
    /// The thread's index in its instance's thread table. `None`
    /// until the thread is registered there: an implicit thread is
    /// registered as its task starts, and an explicit thread as it
    /// is created. A thread of a task that belongs to no instance is
    /// never registered.
    pub index: Option<u32>,
    /// Whether the thread is suspended: it is not running, and it is
    /// not waiting to run. An explicit thread is suspended from its
    /// creation until `thread.resume-later` makes it ready or a
    /// switching built-in starts it. A running thread is suspended by
    /// `thread.suspend` and by the built-ins that suspend and then
    /// switch, until a resume names it.
    pub suspended: bool,
    /// What an explicit thread runs when it starts, until it starts.
    /// `None` for an implicit thread and for an explicit thread that
    /// has started.
    pub start: Option<ThreadStart>,
    /// Whether the thread runs on a stack of its own, under the
    /// provider. Its entry started through the provider, so a blocking
    /// built-in it reaches suspends its stack in the switch module's
    /// shim rather than waiting in a nested turn.
    pub own_stack: bool,
    /// The thread of the same instance whose blocked built-in started
    /// or last resumed this thread through the provider, while that
    /// instance may not suspend. The thread's suspension hands control
    /// back to that built-in's frame, which runs the ready threads of
    /// the instance and nothing else, so the thread may suspend there
    /// even though its instance may not. `None` when the frame that
    /// started or resumed it was anything else: a turn of a driver, or
    /// a thread of another instance or of one that may suspend.
    pub returns_to: Option<ThreadId>,
    /// The waitable set the callback item of this thread's task
    /// takes its event from, while the item is queued to run and
    /// has not taken it. The thread no longer waits then, but the set
    /// still counts it among its waiters, so the set cannot be
    /// dropped under the item, as the reference's callback loop
    /// counts the waiter until it takes its event. `None` when no
    /// such item is queued.
    pub queued_wait: Option<WaitableSetId>,
    /// Whether the thread gives way in a yield that carries the
    /// `cancellable` immediate: `thread.yield`, or a built-in that
    /// yields and then switches. While it is set and the thread is
    /// suspended in the provider, `subtask.cancel` of the thread's
    /// task runs the thread before anything else, as Wasmtime runs a
    /// thread in a cancellable yield.
    pub cancellable_yield: bool,
}

impl Thread {
    /// Construct a thread of `task`, running, with both context
    /// slots zero.
    pub fn new(task: TaskId) -> Self {
        Self {
            task,
            readiness: None,
            context: [0; 2],
            old_may_not_suspend: None,
            index: None,
            suspended: false,
            start: None,
            own_stack: false,
            returns_to: None,
            queued_wait: None,
            cancellable_yield: false,
        }
    }

    /// Construct an explicit thread of `task`, suspended, that runs
    /// `start` when it starts.
    pub fn explicit(task: TaskId, start: ThreadStart) -> Self {
        Self {
            suspended: true,
            start: Some(start),
            ..Self::new(task)
        }
    }
}
