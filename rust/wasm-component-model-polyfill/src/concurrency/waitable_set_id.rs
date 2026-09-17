//! The identity of one waitable set record.

/// The identity of one waitable set: its index in the store's table
/// of waitable sets, together with the generation that slot carried
/// when the record was inserted.
///
/// A guest that holds a waitable set holds a handle-table entry that
/// carries this identity; the identity itself never reaches a guest.
///
/// Indices are per store and a freed index is handed out again, so
/// an index on its own names one set only for as long as that set's
/// record lives. The generation is what makes the identity outlive
/// the index safely: dropping a set advances its slot's generation,
/// so an identity minted for a set that is gone matches no record at
/// all, and in particular never matches the set that takes the index
/// next. A waitable left naming a dropped set would otherwise join
/// whichever set took the index, and a thread parked on one would
/// wait on that set's events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WaitableSetId {
    index: u32,
    generation: u32,
}

impl WaitableSetId {
    /// Name the waitable set record at `index` of generation
    /// `generation`. Workspace-internal: only the store's table of
    /// waitable sets mints one.
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
