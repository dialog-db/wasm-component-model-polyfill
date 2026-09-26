//! What one turn of the store's scheduler achieved.

/// What one turn of the store's scheduler achieved.
///
/// A turn runs the guest work that is ready, polls the host tasks
/// the executor woke, and reports back to the driver that polled it.
/// The driver loops on [`Progress`](Self::Progress), returns pending
/// on [`Waiting`](Self::Waiting), [`Yield`](Self::Yield), and
/// [`Resuming`](Self::Resuming), and
/// applies the idle rule on [`Idle`](Self::Idle): a call or an
/// instantiation fails there, because nothing can make its condition
/// true any more.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The turn left something ready to run, so the driver should
    /// poll again at once.
    Progress,
    /// An item gave way. The scheduler holds it in the
    /// resume-after-yield slot and the driver returns control to the
    /// host executor before it runs.
    Yield,
    /// Nothing is ready and at least one host task is pending. The
    /// driver returns pending; the host task carries the driver's
    /// waker, so the executor wakes it when the host task does.
    Waiting,
    /// Nothing is ready and no host task is pending. Only the
    /// driver's own condition can still be true.
    Idle,
    /// A thread the turn resumed runs on after the turn returned, on
    /// a microtask, and the turn waits for it to stop: it runs
    /// nothing else in between. The driver returns pending, and the
    /// provider wakes it once the thread stopped. Only a provider that
    /// resumes a thread on a microtask, the JSPI provider, leads here.
    Resuming,
}
