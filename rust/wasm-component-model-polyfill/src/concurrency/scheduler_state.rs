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
/// - The in-turn flag a driver consults: a driver entered while
///   another driver of the same store is inside a turn fails with
///   the recursive-driver cause.
pub struct SchedulerState {
    active_waker: Option<Waker>,
    in_turn: bool,
}

impl SchedulerState {
    /// Construct the state of a store with no turn running.
    pub fn new() -> Self {
        Self {
            active_waker: None,
            in_turn: false,
        }
    }

    /// Mark a turn as running, with `waker` as the waker a
    /// trampoline polls a host task with.
    pub fn enter_turn(&mut self, waker: &Waker) {
        self.active_waker = Some(waker.clone());
        self.in_turn = true;
    }

    /// Mark the running turn as over and forget its waker.
    pub fn leave_turn(&mut self) {
        self.active_waker = None;
        self.in_turn = false;
    }

    /// Whether a turn of this store is running.
    pub fn in_turn(&self) -> bool {
        self.in_turn
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
