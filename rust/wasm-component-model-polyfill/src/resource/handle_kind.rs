//! What a handle-table entry names, for every kind a guest can hold.

use crate::concurrency::{EndId, EndKind, ErrorContextId, SubtaskId, TaskId, WaitableSetId};

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
    /// A borrowed resource entry, lowered in for one call. `task` is
    /// the task the borrow is owed to; that task cannot return until
    /// the guest drops the borrow.
    Borrow {
        /// The identity of the resource's type.
        type_id: ResourceTypeId,
        /// Whether a component defines the type (`true`) or the host
        /// does. Only the trap message reads it.
        guest_defined: bool,
        /// The resource's 32-bit representation.
        rep: u32,
        /// The task the borrow is owed to.
        task: TaskId,
    },
    /// A subtask: the identity of its record in the store's subtask
    /// table.
    Subtask {
        /// The subtask the entry names. The identity carries the
        /// generation of the record's slot, so an entry left behind
        /// by a call that has ended goes on naming that call and
        /// never the subtask that took its index.
        subtask: SubtaskId,
    },
    /// A waitable set: the identity of its record in the store's
    /// waitable-set table.
    WaitableSet {
        /// The waitable set the entry names, under the same rule as
        /// the identity a subtask entry carries.
        set: WaitableSetId,
    },
    /// A readable stream end: the identity of its record in the
    /// store's table of ends, under the same rule as the identity a
    /// subtask entry carries.
    StreamReadable {
        /// The end the entry names.
        end: EndId,
    },
    /// A writable stream end, under the same rule as
    /// [`HandleKind::StreamReadable`].
    StreamWritable {
        /// The end the entry names.
        end: EndId,
    },
    /// A readable future end, under the same rule as
    /// [`HandleKind::StreamReadable`].
    FutureReadable {
        /// The end the entry names.
        end: EndId,
    },
    /// A writable future end, under the same rule as
    /// [`HandleKind::StreamReadable`].
    FutureWritable {
        /// The end the entry names.
        end: EndId,
    },
    /// An error context: the identity of its record in the store's
    /// table of error contexts, under the same rule as the identity a
    /// subtask entry carries.
    ErrorContext {
        /// The error context the entry names.
        context: ErrorContextId,
    },
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

    /// The entry of kind `kind` that names the end record `end`.
    pub fn end(kind: EndKind, end: EndId) -> Self {
        match kind {
            EndKind::StreamReadable => Self::StreamReadable { end },
            EndKind::StreamWritable => Self::StreamWritable { end },
            EndKind::FutureReadable => Self::FutureReadable { end },
            EndKind::FutureWritable => Self::FutureWritable { end },
        }
    }

    /// The kind of end and the end record the entry names, for a
    /// stream or future end. `None` for every other kind.
    pub fn as_end(&self) -> Option<(EndKind, EndId)> {
        match *self {
            Self::StreamReadable { end } => Some((EndKind::StreamReadable, end)),
            Self::StreamWritable { end } => Some((EndKind::StreamWritable, end)),
            Self::FutureReadable { end } => Some((EndKind::FutureReadable, end)),
            Self::FutureWritable { end } => Some((EndKind::FutureWritable, end)),
            _ => None,
        }
    }
}
