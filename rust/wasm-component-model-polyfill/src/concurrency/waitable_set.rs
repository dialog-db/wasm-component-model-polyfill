//! One waitable set: the waitables a thread can wait on together.

use super::waitable_id::WaitableId;

/// The record of one waitable set.
///
/// A set holds the list of its waitables and the count of threads
/// waiting on it. The list is kept in join order, because a set
/// delivers pending events in the order its waitables joined it,
/// which is the order Wasmtime delivers them in where the reference
/// leaves the choice to the host.
pub struct WaitableSet {
    /// The waitables that joined the set, in the order they joined.
    pub waitables: Vec<WaitableId>,
    /// How many threads are waiting on the set. The set cannot be
    /// dropped while the count is above zero.
    pub num_waiting: u32,
}

impl WaitableSet {
    /// Construct an empty set that no thread waits on.
    pub fn new() -> Self {
        Self {
            waitables: Vec::new(),
            num_waiting: 0,
        }
    }
}

impl Default for WaitableSet {
    fn default() -> Self {
        Self::new()
    }
}
