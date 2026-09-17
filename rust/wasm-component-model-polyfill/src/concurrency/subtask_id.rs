//! The identity of one subtask record.

/// The identity of one subtask record: its index in the store's
/// table of subtasks, together with the generation that slot carried
/// when the record was inserted.
///
/// A guest that holds a subtask holds a handle-table entry that
/// carries this identity; the identity itself never reaches a guest.
///
/// Indices are per store and a freed index is handed out again, so
/// an index on its own names one subtask only for as long as that
/// subtask's record lives. The generation is what makes the identity
/// outlive the index safely: removing a subtask record advances its
/// slot's generation, so an identity minted for a call that has
/// ended matches no record at all, and in particular never matches
/// the subtask that takes the index next.
///
/// That matters because a subtask is a scope: a borrow lifted out of
/// an owning handle is lent to the scope that lifted it, and the
/// scope is named by this identity. A lend recorded against the
/// identity of a call that has ended would be given back by whichever
/// call took the index, which is the one thing the lend bookkeeping
/// must never do. The generation makes the lend fail instead.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubtaskId {
    index: u32,
    generation: u32,
}

impl SubtaskId {
    /// Name the subtask record at `index` of generation
    /// `generation`. Workspace-internal: only the store's subtask
    /// table mints one.
    pub fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The index this identity names.
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation this identity names. A record table slot
    /// matches the identity only while it carries this generation.
    pub fn generation(self) -> u32 {
        self.generation
    }
}
