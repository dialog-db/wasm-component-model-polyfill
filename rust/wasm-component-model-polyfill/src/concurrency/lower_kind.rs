//! Which lowering a guest called a host function through.

/// Which lowering a guest called a host function through.
///
/// The two differ only in what happens when the host's future is not
/// ready at once. An asynchronous lower hands the call back to the
/// guest as a subtask it can wait on. A synchronous lower has to
/// block the guest thread where it stands, which needs the suspend
/// seam, and fails with the stack-switch cause on a target that has
/// no provider for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerKind {
    /// A `canon lower` without `async`: the guest expects the result
    /// when the call returns.
    Sync,
    /// A `canon lower` with `async`: the guest expects a status word
    /// and waits on the subtask it names.
    Async,
}
