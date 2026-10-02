// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The reading side a host gives a stream it holds.

use core::pin::Pin;
use core::task::{Context, Poll};

use crate::error::Result;
use crate::store::StoreContext;

use super::source::Source;
use super::stream_result::StreamResult;

/// The reading side a host gives a stream it holds. The name and the
/// contract are Wasmtime's.
///
/// [`StreamReader::pipe`](super::StreamReader::pipe) takes a consumer
/// and makes it the reading side of the stream. Whenever the writer
/// starts a write, the store polls the consumer inside a turn, with
/// the store context of the store that owns the end, the [`Source`]
/// of the write, and the `finish` flag. `D` is that store's host data.
///
/// A poll answers in one of these ways:
///
/// - A consumer that can take items reads them from the source and
///   answers [`StreamResult::Completed`] when it can take more, or
///   [`StreamResult::Dropped`] when it cannot. Answering `Completed`
///   when no poll of the write took an item, from a write that
///   offered some, is a failure, because the writer would be told of
///   a write that moved nothing. Wasmtime makes that check only when
///   the writer is the host; the polyfill makes it for a guest writer
///   too, because the Concurrency explainer lets a write of one or
///   more values complete only once one of them moved, and the
///   producer's check is as strict. The items the consumer leaves in
///   the source stay with the writer, and the write reports only the
///   items taken.
/// - A consumer can take items and still answer pending. The write
///   then stays in progress: the writer is not told of it until a
///   later poll answers ready, and that poll is handed a source that
///   offers only the items not yet taken. That is the backpressure of
///   Wasmtime's contract, for a consumer that forwards what it took
///   to a sink that has not accepted it yet. Items taken cannot be
///   put back.
/// - A consumer that can take nothing yet stores the waker of `cx`
///   and answers pending. The store polls it again once the waker is
///   woken.
/// - A write of zero items reaches the consumer as a source whose
///   [`remaining`](Source::remaining) is zero. The guest is asking
///   whether the stream is ready. The consumer can answer `Completed`
///   at once, or answer pending until it can take items and
///   `Completed` then.
/// - `finish` is true when the guest cancelled its write. The
///   consumer must then answer ready as soon as it can: with
///   [`StreamResult::Cancelled`] when it took nothing, and it may
///   answer pending once more to finish work it had started.
///   Answering `Cancelled` when `finish` is false is a failure. Items
///   taken by a poll that answers `Cancelled` count: the write
///   reports them as its progress, as Wasmtime's code counts them.
/// - A poll that answers with an error fails the guest's built-in
///   with that error, as the failure of a host task fails the guest
///   task that called it: the built-in fails at once when the write
///   is polled before it returns, and the task that started the
///   write ends with the error when the failure comes in a later
///   turn.
///
/// A consumer that answers `Dropped` is never polled again: the
/// stream's readable end drops, the write that was polled completes
/// with the dropped result and the progress it made, and a later
/// write sees the dropped result at once. When the writer drops its
/// end instead, the consumer is dropped without another poll, as
/// Wasmtime drops it, so a consumer learns that the stream ended
/// through its own `Drop`.
///
/// Wasmtime gives no type a built-in consumer, and neither does the
/// polyfill.
///
/// The `Send` half of the bound is the one per-target line. It is
/// required natively, so that a store stays `Send`. It is absent in
/// the browser, so that a consumer can await a JavaScript promise,
/// which is not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait StreamConsumer<D: 'static>: Send + 'static {
    /// The type of each item the stream carries.
    type Item;

    /// Take items from the write `source` describes. See the trait's
    /// documentation for the contract.
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}

/// The reading side a host gives a stream it holds. See the native
/// definition for the contract and for why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait StreamConsumer<D: 'static>: 'static {
    /// The type of each item the stream carries.
    type Item;

    /// Take items from the write `source` describes. See the native
    /// definition for the contract.
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}
