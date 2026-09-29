//! The identity of one stream or future end record.

/// The identity of one stream or future end record: its index in the
/// store's table of ends, together with the generation that slot
/// carried when the record was inserted.
///
/// A guest that holds an end holds a handle-table entry that carries
/// this identity, as a subtask entry carries the identity of its
/// record; the identity itself never reaches a guest.
///
/// Indices are per store and a freed index is handed out again, so an
/// index on its own names one end only for as long as that end's
/// record lives. The generation is what makes the identity outlive
/// the index safely: removing an end record advances its slot's
/// generation, so an identity minted for an end that is gone matches
/// no record at all, and in particular never matches the end that
/// takes the index next.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EndId {
    index: u32,
    generation: u32,
}

impl EndId {
    /// The identity a host value holds once it closed its end, which
    /// names no end record: no table reaches the index. Wasmtime's
    /// close leaves the same kind of identity, `u32::MAX`, in its
    /// value, so every later use of the value fails as the lookup of
    /// an end that is not there.
    pub const CLOSED: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
    };

    /// Name the end record at `index` of generation `generation`.
    /// Workspace-internal: only the store's table of ends mints one.
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
