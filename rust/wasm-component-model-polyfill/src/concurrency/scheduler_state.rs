//! The half of the scheduler a trampoline can reach.

use core::task::Waker;

/// The half of the scheduler a trampoline can reach.
///
/// The ready queues and the host tasks live on the store, where the
/// type of the host data is known and where nothing has to be `Send`
/// in the browser. Two things a trampoline needs do not: it reaches
/// the store's handle tables from inside a runtime-layer closure,
/// and it finds them here.
///
/// - The waker of the turn that is running. A trampoline that starts
///   a host task polls its future once before it returns to the
///   guest, and it must poll with the waker of the active turn so
///   that no wake is lost. There is no waker when no turn is
///   running; the trampoline then polls with a waker that does
///   nothing, and the next turn polls again.
/// - Whether a turn is running, which a driver consults: a driver
///   entered while another driver of the same store is inside a turn
///   fails with the recursive-driver cause.
///
/// A turn can nest inside another: a host task's body reaches the
/// store through the accessor the poll hands it, and the closure it
/// runs there is guest work, so it runs inside a turn of its own.
/// The state therefore counts the turns that are running and hands
/// each one the waker it displaced back when it ends.
pub struct SchedulerState {
    active_waker: Option<Waker>,
    depth: usize,
}

impl SchedulerState {
    /// Construct the state of a store with no turn running.
    pub fn new() -> Self {
        Self {
            active_waker: None,
            depth: 0,
        }
    }

    /// Mark a turn as running, with `waker` as the waker a
    /// trampoline polls a host task with. The waker the turn
    /// displaced comes back, for [`leave_turn`](Self::leave_turn) to
    /// restore when this turn ends.
    pub fn enter_turn(&mut self, waker: &Waker) -> Option<Waker> {
        self.depth += 1;
        self.active_waker.replace(waker.clone())
    }

    /// Mark the running turn as over and put `displaced` back as the
    /// waker of the turn it was nested in, if it was nested in one.
    pub fn leave_turn(&mut self, displaced: Option<Waker>) {
        self.depth = self.depth.saturating_sub(1);
        self.active_waker = displaced;
    }

    /// Whether a turn of this store is running.
    pub fn in_turn(&self) -> bool {
        self.depth > 0
    }

    /// The waker of the running turn, or `None` when no turn is
    /// running.
    pub fn active_waker(&self) -> Option<Waker> {
        self.active_waker.clone()
    }
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self::new()
    }
}
