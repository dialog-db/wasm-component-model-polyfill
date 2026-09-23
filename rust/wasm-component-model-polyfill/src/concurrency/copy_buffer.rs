//! The buffer of one copy in progress on a guest end.

/// The buffer of one copy in progress on a guest end: where in the
/// guest's memory the values are read from or written to, how many
/// the copy asked for, and how many it has moved so far.
///
/// An end holds one while it is copying and none otherwise. Nothing
/// starts a copy yet, so nothing builds one: the built-ins that read
/// and write an end fill the slot, and the pairing of two copies
/// reads it.
#[allow(dead_code)]
pub struct CopyBuffer {
    /// The guest's pointer to the first value of the copy.
    pub pointer: u32,
    /// The count of values the copy asked for.
    pub length: u32,
    /// The count of values the copy has moved so far.
    pub progress: u32,
}
