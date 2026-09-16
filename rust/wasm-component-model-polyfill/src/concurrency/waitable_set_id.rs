//! The identity of one waitable set record.

/// The identity of one waitable set: its index in the store's table
/// of waitable sets.
///
/// A guest that holds a waitable set holds a handle-table entry that
/// carries this index; the identity itself never reaches a guest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WaitableSetId(u32);

impl WaitableSetId {
    /// Name the waitable set record at `index`. Workspace-internal:
    /// only the store's table of waitable sets mints one.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.0
    }
}
