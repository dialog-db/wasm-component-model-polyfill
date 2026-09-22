//! What one item of the scheduler's ready queues is.

/// What one item of the scheduler's ready queues is.
///
/// The scheduler runs an item to its next yield point and then looks
/// at its queues again. The kind is what the item does when it runs;
/// it does not affect which queue the item sits in, and the
/// scheduler never branches on it. It is carried so that a reader of
/// a queue — a test, or a later feature that reports what the store
/// is holding — can say what is waiting to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    /// The start of a task: the implicit thread of a call into an
    /// export runs for the first time.
    TaskStart,
    /// A callback invocation: the polyfill re-enters the callback of
    /// an `async` export that returned a status code.
    Callback,
    /// The resumption of a thread the scheduler suspended. Nothing
    /// queues one under this label yet.
    #[allow(dead_code)]
    ThreadResumption,
    /// The lowering of a completed host task's result into the
    /// subtask that awaits it.
    HostResultLowering,
}
