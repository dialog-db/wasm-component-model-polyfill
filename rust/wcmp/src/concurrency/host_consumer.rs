// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A host consumer as the store drives it, still typed by its items.

use core::task::{Context, Poll};

use crate::error::Result;
use crate::linker::ComponentValue;
use crate::store::StoreContext;

use super::source::Source;

/// A [`StreamConsumer`](super::StreamConsumer) or a
/// [`FutureConsumer`](super::FutureConsumer), wrapped so that the
/// store drives both the same way: one poll per [`consume`], with the
/// contract of the consumer's trait checked on the answer.
///
/// The store drives one of these for a guest's write, erased to a
/// [`HostReader`](super::host_reader::HostReader), and for a stream or
/// future the host created and piped to itself, where no guest is
/// involved and the items come from the host's producer.
///
/// [`consume`]: Self::consume
///
/// The `Send` half of the bound is the one per-target line, as it is
/// for the consumer the end wraps.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostConsumer<D: 'static>: Send + 'static {
    /// The type of each item the consumer takes.
    type Item: ComponentValue;

    /// Poll the consumer once with `source`, unless it is finished, in
    /// which case the poll is ready at once. `earlier` is the count of
    /// items the same write lost to earlier polls that answered
    /// pending, which the check of a completed answer counts as taken.
    /// A poll that comes out ready answered as the contract allows.
    fn consume(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
        earlier: usize,
    ) -> Poll<Result<()>>;

    /// Whether the consumer will take nothing more: a stream's answered
    /// that it is over, or a future's took its value.
    fn finished(&self) -> bool;
}

/// A host consumer as the store drives it. See the native definition
/// for what it is and why the `Send` half of the bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostConsumer<D: 'static>: 'static {
    /// The type of each item the consumer takes.
    type Item: ComponentValue;

    /// Poll the consumer once. See the native definition.
    fn consume(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        source: Source<'_, Self::Item>,
        finish: bool,
        earlier: usize,
    ) -> Poll<Result<()>>;

    /// Whether the consumer will take nothing more.
    fn finished(&self) -> bool;
}
