//! The identity of one task record.

/// The identity of one task record: its index in the store's table
/// of tasks.
///
/// Indices are per store, and a freed index is handed out again: the
/// table of tasks keeps no generation beside the index, so an
/// identity names one task only for as long as that task's record
/// lives. A borrow entry in a handle table names the task the borrow
/// is owed to by this identity, and what keeps the entry from naming
/// a later task that took the same index is the count the task's exit
/// checks: a task that ends with borrows outstanding fails its call,
/// so no borrow entry of a call that returned outlives the task it
/// was owed to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(u32);

impl TaskId {
    /// Name the task record at `index`. Workspace-internal: only the
    /// store's task table mints one.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.0
    }
}
