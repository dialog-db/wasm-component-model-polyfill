//! The runtime record of one component instance.

use super::thread_id::ThreadId;

/// The runtime record of one component instance: the fields of the
/// reference's `ComponentInstance` that the runtime needs beyond the
/// instance's handle table.
///
/// The entry gate reads and writes the first three fields, and the
/// enter and exit intrinsics of an adapter maintain
/// `may_not_suspend`.
///
/// The instance's may-leave flag is not here. It is the core global
/// the instance's fused adapters read and write, which the
/// instantiation's canonical-ABI runtime state holds, so that the
/// built-ins and the generated code read one flag rather than two.
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
    /// Whether a thread running in this instance is forbidden to
    /// suspend. The enter intrinsic sets it for the duration of a
    /// synchronous call and the exit intrinsic restores it.
    pub may_not_suspend: bool,
}

impl InstanceRecord {
    /// Construct the record of a fresh instance: no backpressure, no
    /// queued task, no exclusive thread, and free to suspend.
    pub fn new() -> Self {
        Self {
            backpressure: 0,
            waiting_to_enter: 0,
            exclusive_thread: None,
            may_not_suspend: false,
        }
    }
}

impl Default for InstanceRecord {
    fn default() -> Self {
        Self::new()
    }
}
