//! The identity of one waitable.

use super::subtask_id::SubtaskId;

/// The identity of one waitable: a handle a guest can wait on.
///
/// A waitable is a subtask, a readable or writable stream end, or a
/// readable or writable future end. The identity names the record the
/// waitable's state lives on, so every waitable operation of the
/// store takes one of these.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WaitableId {
    /// A subtask, by its index in the store's table of subtasks.
    Subtask(SubtaskId),
    /// A readable stream end, by its index in the store's table of
    /// stream ends. Reserved for the feature that defines streams;
    /// nothing constructs this variant yet.
    #[allow(dead_code)]
    StreamReadable(u32),
    /// A writable stream end, under the same rule as
    /// [`WaitableId::StreamReadable`].
    #[allow(dead_code)]
    StreamWritable(u32),
    /// A readable future end, by its index in the store's table of
    /// future ends. Reserved for the feature that defines futures;
    /// nothing constructs this variant yet.
    #[allow(dead_code)]
    FutureReadable(u32),
    /// A writable future end, under the same rule as
    /// [`WaitableId::FutureReadable`].
    #[allow(dead_code)]
    FutureWritable(u32),
}
