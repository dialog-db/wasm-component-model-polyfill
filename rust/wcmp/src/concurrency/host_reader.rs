// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The readable end of a stream or a future the host serves.

use core::task::{Context, Poll};

use crate::error::Result;
use crate::internal::SourceInternal;
use crate::store::StoreContext;

use super::copy_buffer::CopyBuffer;
use super::host_consumer::HostConsumer;
use super::source::Source;

/// The readable end of a stream or a future the host serves through a
/// consumer, as the store holds it.
///
/// The end records of the store hold no host data type, so they
/// cannot hold a consumer, which is polled with the store's context.
/// The scheduler holds this beside them instead, under the identity
/// of the readable end, for as long as the consumer can still take
/// items. A writable end whose readable end is one of these belongs
/// to a stream or future a guest created, whose readable end the host
/// lifted and then piped with
/// [`StreamReader::pipe`](super::StreamReader::pipe) or
/// [`FutureReader::pipe`](super::FutureReader::pipe).
///
/// The store polls it when a guest starts a write, and again in later
/// turns while the poll is pending. Each poll takes what the consumer
/// reads straight out of the guest's memory, so, unlike a host
/// writer, nothing waits with the end between polls. A poll that
/// comes out ready is followed by the completion of the write.
///
/// The `Send` half of the bound is the one per-target line, as it is
/// for the consumer the end wraps.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostReader<D: 'static>: Send + 'static {
    /// Poll the consumer for the guest's write `write`, the writer's
    /// buffer as it stood when the poll began, and count the items the
    /// consumer took in `taken`.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        write: &CopyBuffer,
        taken: &mut u32,
        finish: bool,
    ) -> Poll<Result<()>>;

    /// Whether the consumer will take nothing more. An end whose
    /// consumer is finished is over once the write that finished it
    /// completes.
    fn finished(&self) -> bool;
}

/// The readable end of a stream or a future the host serves. See the
/// native definition for what it is and why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostReader<D: 'static>: 'static {
    /// Poll the consumer. See the native definition.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        write: &CopyBuffer,
        taken: &mut u32,
        finish: bool,
    ) -> Poll<Result<()>>;

    /// Whether the consumer will take nothing more.
    fn finished(&self) -> bool;
}

impl<D: 'static, H: HostConsumer<D>> HostReader<D> for H {
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        write: &CopyBuffer,
        taken: &mut u32,
        finish: bool,
    ) -> Poll<Result<()>> {
        // The items earlier polls of the same write took are the
        // progress the writer's buffer records: a write starts with
        // none, and only the polls of this end add to it.
        let earlier = write.progress as usize;
        self.consume(cx, store, Source::guest(write, taken), finish, earlier)
    }

    fn finished(&self) -> bool {
        HostConsumer::finished(self)
    }
}
