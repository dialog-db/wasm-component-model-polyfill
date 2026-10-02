// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How one poll of a host stream end came out.

/// How one poll of a host stream end came out, when it came out
/// ready. The name and the three cases are Wasmtime's.
///
/// A producer answers with one of these from
/// [`StreamProducer::poll_produce`](super::StreamProducer::poll_produce).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StreamResult {
    /// The poll delivered what it could and the end can go on: a
    /// producer that answers this is polled again for a later read.
    Completed,
    /// The poll was asked to finish, because the guest cancelled its
    /// copy, and it delivered nothing. A producer answers this only
    /// when the `finish` flag of the poll is set. Items it stored
    /// anyway still reach the reader, as in Wasmtime's code.
    Cancelled,
    /// The end is over. A producer that answers this is never polled
    /// again. Items it delivered in the same poll still reach the
    /// reader, and the reader learns that the stream ended once it
    /// has taken them all.
    Dropped,
}
