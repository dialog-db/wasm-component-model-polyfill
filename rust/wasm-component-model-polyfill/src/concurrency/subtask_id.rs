//! The identity of one subtask record.

/// The identity of one subtask record: its index in the store's
/// table of subtasks.
///
/// A guest that holds a subtask holds a handle-table entry that
/// carries this index; the identity itself never reaches a guest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubtaskId(u32);

impl SubtaskId {
    /// Name the subtask record at `index`. Workspace-internal: only
    /// the store's subtask table mints one.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.0
    }
}
