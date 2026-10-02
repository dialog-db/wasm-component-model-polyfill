// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The reading side a host gives a future it holds.

use core::pin::Pin;
use core::task::{Context, Poll};

use crate::error::Result;
use crate::store::StoreContext;

use super::source::Source;

/// The reading side a host gives a future it holds. The name and the
/// contract are Wasmtime's.
///
/// [`FutureReader::pipe`](super::FutureReader::pipe) takes a consumer
/// and makes it the reading side of the future. When the writer
/// writes the value, the store polls the consumer inside a turn, with
/// the store context of the store that owns the end, the [`Source`]
/// of the write, which offers the one value, and the `finish` flag.
/// `D` is that store's host data.
///
/// The contract is the one [`StreamConsumer`](super::StreamConsumer)
/// states, narrowed to one value. A consumer that takes the value
/// from the source and answers ready has received it: the write
/// completes, and the consumer is never polled again. A consumer
/// that cannot take it yet stores the waker and answers pending, and
/// it may take the value and still answer pending, which delays the
/// write's completion until a later poll answers ready. `finish` is
/// true when the guest cancelled its write, and the consumer may then
/// answer ready without taking the value: the write ends cancelled,
/// the value stays with the writer, and the next write polls the
/// consumer again. Answering ready without taking the value when
/// `finish` is false is a failure. A poll that answers with an error
/// fails the guest's built-in with that error.
///
/// When the writer drops its end before it writes, the consumer is
/// dropped without another poll, as Wasmtime drops it.
///
/// The `Send` half of the bound is the one per-target line: required
/// natively, absent in the browser, for the reason the stream
/// consumer states.
#[cfg(not(target_arch = "wasm32"))]
pub trait FutureConsumer<D: 'static>: Send + 'static {
    /// The type of the value the future carries.
    type Item;

    /// Take the future's value from `source`. See the trait's
    /// documentation for the contract.
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<()>>;
}

/// The reading side a host gives a future it holds. See the native
/// definition for the contract and for why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait FutureConsumer<D: 'static>: 'static {
    /// The type of the value the future carries.
    type Item;

    /// Take the future's value from `source`. See the native
    /// definition for the contract.
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<()>>;
}
