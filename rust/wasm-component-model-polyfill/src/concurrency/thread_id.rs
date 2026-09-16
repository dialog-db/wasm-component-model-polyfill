//! The identity of one thread record.

/// The identity of one thread record: its index in the store's table
/// of threads.
///
/// Every task has at least one thread, its implicit thread. A thread
/// is one guest execution: it carries the context slots
/// `context.get` and `context.set` read and write, and the readiness
/// condition it waits on while it is suspended.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadId(u32);

impl ThreadId {
    /// Name the thread record at `index`. Workspace-internal: only
    /// the store's thread table mints one.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.0
    }
}
