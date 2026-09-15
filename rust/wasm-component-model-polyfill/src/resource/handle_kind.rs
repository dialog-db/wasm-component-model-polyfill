//! Whether a handle-table entry owns its resource or borrows it.

/// The ownership of a live handle-table entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleKind {
    /// An owning entry. `lend_count` counts the borrows currently
    /// lifted out of it into a call; an owned entry cannot be removed
    /// while that count is above zero.
    Own { lend_count: u32 },
    /// A borrow lowered in for one call. `scope` is the position on
    /// the store's call stack the borrow belongs to; the call cannot
    /// end until the guest drops the borrow.
    Borrow { scope: usize },
}
