//! A thread whose stop the scheduler waits for.

use super::parked_thread::ParkedThread;
use super::thread_id::ThreadId;

/// A thread whose stop the scheduler waits for, under a provider that
/// runs a thread on after the call that started or resumed it
/// returned: a thread the JSPI provider resumed, which runs on a
/// microtask, or one whose start failed and whose trap the browser
/// hands over on a microtask.
///
/// The scheduler waits for one such thread at a time, and runs nothing
/// else until it stops. The turn that made the resume ends there, and
/// the driver returns control to the host executor, which runs the
/// microtask. The provider wakes the driver once the thread stopped,
/// and the next turn takes up where the last one stopped.
pub struct InFlight<T: 'static> {
    /// The thread.
    pub thread: ThreadId,
    /// The thread as it was parked, with the finish its starter left.
    pub parked: ParkedThread<T>,
    /// Where the thread's scopes begin on the stack of current scopes,
    /// once they are back on it. A thread that was resumed has them
    /// back already. One whose start failed gets them back when its
    /// trap arrives, and runs its finish over them.
    pub base: Option<usize>,
}
