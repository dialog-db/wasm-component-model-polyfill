// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The identity of one waitable.

use super::end_id::EndId;
use super::end_kind::EndKind;
use super::subtask_id::SubtaskId;

/// The identity of one waitable: a handle a guest can wait on.
///
/// A waitable is a subtask, a readable or writable stream end, or a
/// readable or writable future end. The identity names the record the
/// waitable's state lives on, so every waitable operation of the
/// store takes one of these.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WaitableId {
    /// A subtask, by the identity of its record in the store's table
    /// of subtasks.
    Subtask(SubtaskId),
    /// A readable stream end, by the identity of its record in the
    /// store's table of ends.
    StreamReadable(EndId),
    /// A writable stream end, under the same rule as
    /// [`WaitableId::StreamReadable`].
    StreamWritable(EndId),
    /// A readable future end, under the same rule as
    /// [`WaitableId::StreamReadable`].
    FutureReadable(EndId),
    /// A writable future end, under the same rule as
    /// [`WaitableId::StreamReadable`].
    FutureWritable(EndId),
}

impl WaitableId {
    /// The waitable that `end`, an end of kind `kind`, is.
    pub fn from_end(kind: EndKind, end: EndId) -> Self {
        match kind {
            EndKind::StreamReadable => Self::StreamReadable(end),
            EndKind::StreamWritable => Self::StreamWritable(end),
            EndKind::FutureReadable => Self::FutureReadable(end),
            EndKind::FutureWritable => Self::FutureWritable(end),
        }
    }

    /// The kind of end and the end record the waitable names, for a
    /// stream or future end. `None` for a subtask.
    pub fn end(self) -> Option<(EndKind, EndId)> {
        match self {
            Self::Subtask(_) => None,
            Self::StreamReadable(end) => Some((EndKind::StreamReadable, end)),
            Self::StreamWritable(end) => Some((EndKind::StreamWritable, end)),
            Self::FutureReadable(end) => Some((EndKind::FutureReadable, end)),
            Self::FutureWritable(end) => Some((EndKind::FutureWritable, end)),
        }
    }
}
