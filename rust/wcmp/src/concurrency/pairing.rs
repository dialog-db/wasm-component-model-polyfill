// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a copy that just started on a stream or future end asks of
//! the caller.

use super::end_id::EndId;

/// What a copy that just started on a stream or future end asks of
/// the caller, once the store's records have paired it with the other
/// end.
///
/// The records decide how a read and a write meet, which is the
/// reference's `SharedStreamImpl.read` and `write`, and its
/// `SharedFutureImpl`'s for one value. Moving the values is not
/// theirs to do: it reads one guest's memory and writes another's
/// through two boundary contexts, and those need the store, which the
/// records sit inside. So the pairing answers which values move, the
/// caller moves them, and the caller then reports the move back to
/// the records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pairing {
    /// No value moves. The copy became the pending side, or completed
    /// at once with nothing moved, or found the other end dropped. The
    /// end's pending event says which, when it holds one.
    Settled,
    /// `count` values move now, out of the writable end's buffer and
    /// into the readable end's, each from where its copy has got to.
    Move {
        /// The writable end, whose buffer the values are read from.
        writer: EndId,
        /// The readable end, whose buffer the values are written to.
        reader: EndId,
        /// How many values move: the smaller of the two counts the
        /// buffers can still take.
        count: u32,
    },
}
