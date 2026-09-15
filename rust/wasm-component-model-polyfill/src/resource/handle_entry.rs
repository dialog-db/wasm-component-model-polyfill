//! One live entry of a handle table.

/// What a live handle-table index refers to: a resource the table
/// owns, or a borrow of one lent for the duration of a call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleEntry {
    /// An owning entry. `lend_count` counts the borrows currently
    /// lifted out of it into a host call; an owned entry cannot be
    /// removed while that count is above zero.
    Own { rep: u32, lend_count: u32 },
    /// A borrow lowered into the guest for one call. `scope` is the
    /// position on the store's call stack the borrow belongs to; the
    /// call cannot end until the guest drops the borrow.
    Borrow { rep: u32, scope: usize },
}

impl HandleEntry {
    /// The resource's 32-bit representation.
    pub fn rep(&self) -> u32 {
        match self {
            HandleEntry::Own { rep, .. } | HandleEntry::Borrow { rep, .. } => *rep,
        }
    }
}
