// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The writable end of a stream or a future the host serves.

use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::error::Result;
use crate::store::{StoreContext, StoreData};
use crate::types::ValueType;
use crate::value::Val;

/// The writable end of a stream or a future the host serves through a
/// producer, as the store holds it.
///
/// The end records of the store hold no host data type, so they
/// cannot hold a producer, which is polled with the store's context.
/// The scheduler holds this beside them instead, under the identity
/// of the writable end, for as long as the producer can still
/// deliver. A readable end whose writable end is one of these belongs
/// to a stream or future created by
/// [`StreamReader::new`](super::StreamReader::new) or
/// [`FutureReader::new`](super::FutureReader::new).
///
/// The end keeps what the producer delivered and the reader has not
/// taken yet, so a poll happens only when nothing is waiting. The
/// store polls it when a guest starts a read, and again in later
/// turns while the poll is pending; each poll that comes out ready is
/// followed by a delivery, which moves what the reader can take into
/// the reader's buffer through the boundary context of the read.
///
/// When the host pipes the readable end of such a stream or future to
/// a consumer of its own, the pipe takes the end out of the scheduler
/// and polls it with no remaining count, and takes what it delivered
/// as values instead of lowering them into a guest.
///
/// The `Send` half of the bound is the one per-target line, as it is
/// for the producer the end wraps.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostWriter<D: 'static>: Send + 'static {
    /// Poll the producer for a read that can take `remaining` items,
    /// unless items or the end of the stream are already waiting, in
    /// which case the poll is ready at once. A poll that comes out
    /// ready has left what it produced with the end. `remaining` is
    /// `None` when the reader is a consumer of the host, which takes
    /// whatever the producer delivers.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        remaining: Option<usize>,
        finish: bool,
    ) -> Poll<Result<()>>;

    /// How many items are waiting for the reader.
    fn waiting(&self) -> usize;

    /// Whether the producer will deliver nothing more. An end whose
    /// producer is finished and which has no item waiting is over.
    fn finished(&self) -> bool;

    /// Lower the first `count` waiting items, each a value of `ty`,
    /// into the reader's memory from `offset` on, through `cx`.
    fn deliver(
        &mut self,
        cx: &mut BoundaryContext<'_, StoreData<D>>,
        offset: usize,
        ty: &ValueType,
        count: usize,
    ) -> Result<()>;

    /// Take every waiting item, each as the value it crosses as, for a
    /// reader that is a consumer of the host rather than a guest.
    fn take_items(&mut self) -> Vec<Val>;
}

/// The writable end of a stream or a future the host serves. See the
/// native definition for what it is and why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostWriter<D: 'static>: 'static {
    /// Poll the producer. See the native definition.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        remaining: Option<usize>,
        finish: bool,
    ) -> Poll<Result<()>>;

    /// How many items are waiting for the reader.
    fn waiting(&self) -> usize;

    /// Whether the producer will deliver nothing more.
    fn finished(&self) -> bool;

    /// Lower the first `count` waiting items. See the native
    /// definition.
    fn deliver(
        &mut self,
        cx: &mut BoundaryContext<'_, StoreData<D>>,
        offset: usize,
        ty: &ValueType,
        count: usize,
    ) -> Result<()>;

    /// Take every waiting item. See the native definition.
    fn take_items(&mut self) -> Vec<Val>;
}
