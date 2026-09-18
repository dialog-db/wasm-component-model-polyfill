//! The runtime record of one component instance.

use super::thread_id::ThreadId;

/// The runtime record of one component instance: the fields of the
/// reference's `ComponentInstance` that the runtime needs beyond the
/// instance's handle table.
///
/// The entry gate reads and writes the first three fields, and the
/// enter and exit intrinsics of an adapter maintain
/// `may_not_suspend`. `may_leave` is clear for the length of a call
/// the polyfill itself makes into the guest.
pub struct InstanceRecord {
    /// How many times the guest has raised backpressure without
    /// lowering it again. A task cannot enter the instance while the
    /// count is above zero. The entry gate reads it, and the
    /// `backpressure.inc` and `backpressure.dec` built-ins raise and
    /// lower it: a raise past the sixteen bits the reference counts
    /// backpressure in, or a lowering below zero, traps.
    pub backpressure: u32,
    /// How many tasks are queued at the instance's entry gate. A
    /// fresh task queues behind them rather than overtaking them,
    /// so the tasks of one instance start in arrival order.
    pub waiting_to_enter: u32,
    /// The thread that holds the instance exclusively, when one
    /// does. A task lifted synchronously and a callback task each
    /// need it, and the entry gate hands it over as such a task
    /// starts.
    pub exclusive_thread: Option<ThreadId>,
    /// Whether the instance may be left, which the reference clears
    /// while a call the polyfill itself makes into the guest runs:
    /// the `cabi_realloc` a crossing asks for memory with, and the
    /// `post-return` of an export. A built-in that reads the flag
    /// traps with the cannot-leave cause while it is clear. The
    /// adapters read and write the flags global they compile
    /// against, which is a second copy of the same state.
    pub may_leave: bool,
    /// Whether a thread running in this instance is forbidden to
    /// suspend. The enter intrinsic sets it for the duration of a
    /// synchronous call and the exit intrinsic restores it.
    pub may_not_suspend: bool,
}

impl InstanceRecord {
    /// Construct the record of a fresh instance: no backpressure, no
    /// queued task, no exclusive thread, leavable, and free to
    /// suspend.
    pub fn new() -> Self {
        Self {
            backpressure: 0,
            waiting_to_enter: 0,
            exclusive_thread: None,
            may_leave: true,
            may_not_suspend: false,
        }
    }
}

impl Default for InstanceRecord {
    fn default() -> Self {
        Self::new()
    }
}
