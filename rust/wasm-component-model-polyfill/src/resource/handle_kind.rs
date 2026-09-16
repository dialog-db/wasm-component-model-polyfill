//! What a handle-table entry names, for every kind a guest can hold.

use super::identity::ResourceTypeId;

/// The kind of a live handle-table entry.
///
/// A component instance keeps one handle table, shared by every
/// resource type and every other handle kind the instance uses. Each
/// entry carries the fields its kind needs; the resource fields
/// (`type_id`, `guest_defined`, `rep`) belong only to the two
/// resource kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleKind {
    /// An owning resource entry. `lend_count` counts the borrows
    /// currently lifted out of it into a call; an owned entry cannot
    /// be removed while that count is above zero.
    Own {
        /// The identity of the resource's type.
        type_id: ResourceTypeId,
        /// Whether a component defines the type (`true`) or the host
        /// does. Only the trap message reads it.
        guest_defined: bool,
        /// The resource's 32-bit representation.
        rep: u32,
        /// The count of borrows lent from this entry to calls still
        /// in flight.
        lend_count: u32,
    },
    /// A borrowed resource entry, lowered in for one call. `scope` is
    /// the position on the store's call stack the borrow belongs to;
    /// the call cannot end until the guest drops the borrow.
    Borrow {
        /// The identity of the resource's type.
        type_id: ResourceTypeId,
        /// Whether a component defines the type (`true`) or the host
        /// does. Only the trap message reads it.
        guest_defined: bool,
        /// The resource's 32-bit representation.
        rep: u32,
        /// The position on the store's call stack the borrow is owed
        /// to.
        scope: usize,
    },
    /// A subtask: the index of its record in the store's subtask
    /// table.
    Subtask {
        /// The subtask's index in the store.
        index: u32,
    },
    /// A waitable set: the index of its record in the store's
    /// waitable-set table.
    WaitableSet {
        /// The waitable set's index in the store.
        index: u32,
    },
    /// A readable stream end. Reserved for the feature that defines
    /// streams; nothing constructs this variant yet.
    #[allow(dead_code)]
    StreamReadable,
    /// A writable stream end. Reserved for the feature that defines
    /// streams; nothing constructs this variant yet.
    #[allow(dead_code)]
    StreamWritable,
    /// A readable future end. Reserved for the feature that defines
    /// futures; nothing constructs this variant yet.
    #[allow(dead_code)]
    FutureReadable,
    /// A writable future end. Reserved for the feature that defines
    /// futures; nothing constructs this variant yet.
    #[allow(dead_code)]
    FutureWritable,
    /// An error context. Reserved for the feature that defines error
    /// contexts; nothing constructs this variant yet.
    #[allow(dead_code)]
    ErrorContext,
}

impl HandleKind {
    /// The resource's rep, for an owning or borrowed entry. `None`
    /// for every other kind.
    pub fn rep(&self) -> Option<u32> {
        match *self {
            Self::Own { rep, .. } | Self::Borrow { rep, .. } => Some(rep),
            _ => None,
        }
    }
}
