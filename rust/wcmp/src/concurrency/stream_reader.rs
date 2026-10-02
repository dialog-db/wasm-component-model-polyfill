// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The readable end of a stream the host holds.

use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::abi::layout::size_of;
use crate::error::{AbiPosition, CopyCause, Error, Result};
use crate::executor::{close_readable_end, pipe_readable_end};
use crate::internal::{DestinationInternal, StreamReaderInternal};
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt, StoreData};
use crate::types::ValueType;
use crate::value::Val;

use super::accessor::Accessor;
use super::destination::Destination;
use super::end_id::EndId;
use super::end_kind::EndKind;
use super::guarded_stream_reader::GuardedStreamReader;
use super::host_consumer::HostConsumer;
use super::host_writer::HostWriter;
use super::source::Source;
use super::stream_any::StreamAny;
use super::stream_consumer::StreamConsumer;
use super::stream_producer::StreamProducer;
use super::stream_result::StreamResult;

/// The readable end of a `stream<T>` the host holds. The name is
/// Wasmtime's.
///
/// A host creates a stream with [`StreamReader::new`], giving it the
/// producer that writes it, and hands the reader to a guest: as the
/// result of a typed host function, or as an argument of a typed
/// call. The reader implements
/// [`ComponentValue`](crate::ComponentValue) for that, and lowering
/// it into a guest enters a readable end in the guest's handle table
/// that shares the stream with the producer. The guest then reads
/// the stream with `stream.read`, and each read polls the producer
/// inside a turn of the store.
///
/// A host receives a reader the other way: a stream in the result of
/// a typed call, or in a parameter of a typed host function, arrives
/// as one, and the guest's entry for the end is gone. The lift checks
/// that the guest's stream carries the type `T` projects to. The host
/// then reads the stream with [`pipe`](Self::pipe), which makes a
/// [`StreamConsumer`] its reading side, and each write of the guest
/// polls the consumer inside a turn of the store.
///
/// There is no writer type. A host writes a stream by supplying a
/// producer when it creates the stream.
///
/// The lifecycle rule is Wasmtime's: a reader the host holds must be
/// lowered into a guest, piped, or closed with
/// [`close`](Self::close). The reader has no `Drop` of its own,
/// because its end lives in the store, which the reader cannot
/// reach, so a reader dropped otherwise leaks its end until the store
/// drops, and a guest that writes to the stream waits for good.
/// [`guard`](Self::guard) pairs a reader with an accessor into a
/// [`GuardedStreamReader`], which closes the stream when it drops
/// inside a poll of its store.
///
/// A reader belongs to the store it was created or lifted in, and is
/// lowered, piped, or closed only in that store. It names its end by
/// an index and a generation and carries no identity of the store, so
/// the lower, the pipe, and the close cannot tell a reader from
/// another store. Using a reader with a store other than the one that
/// made it is a host error the polyfill does not detect, as with a
/// [`ResourceHandle`](crate::ResourceHandle): when the other store
/// holds a readable end for the host under the same index and
/// generation, that end is the one lowered, piped, or closed, and the
/// guest reads, the consumer takes, or the close ends the other
/// store's stream. The close needs no identity of the store to keep
/// its own contract, so none is added. Wasmtime's
/// reader carries no store identity either.
///
/// The reader is moved when it is lowered or piped, but it is not the
/// only value that can name its end: each reader decoded from the
/// same [`Val`], or converted from the same untyped value, names it
/// too, as Wasmtime's copies of an id do. Once one of them lowers,
/// pipes, or closes the end, the others are refused those uses, as
/// [`CopyCause::NotHeldByHost`] states, except that a close of an end
/// closed already succeeds and does nothing, and that an end another
/// value piped to a consumer while a guest holds the writable end may
/// be piped again or closed while no write of it is in flight.
pub struct StreamReader<T> {
    end: EndId,
    item: PhantomData<fn() -> T>,
}

impl<T: ComponentValue> StreamReader<T> {
    /// Create a stream in the store `store` reaches, whose writable
    /// end is `producer`, and return its readable end.
    ///
    /// The stream carries values of the type `T` projects to. `D` is
    /// the store's host data. [`Store::as_context_mut`] reaches the
    /// context from a store the host holds, and a host `async`
    /// function reaches it through [`Accessor::with`].
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn new<D: 'static>(
        store: &mut StoreContext<'_, D>,
        producer: impl StreamProducer<D, Item = T>,
    ) -> Result<Self> {
        let (readable, writable) = store
            .internal()
            .lock_tables()?
            .tasks
            .insert_host_ends(Some(T::value_type()), EndKind::StreamWritable)?;
        store.internal().scheduler_mut().insert_host_writer(
            writable,
            Box::new(ProducerEnd {
                producer: Box::pin(producer),
                waiting: Vec::new(),
                dropped: false,
                data: PhantomData,
            }),
        );
        Ok(Self::from_end(readable))
    }

    /// Make `consumer` the reading side of this stream, in the store
    /// `store` reaches, and give the reader up. The name and the
    /// contract are Wasmtime's.
    ///
    /// From then on each write the writer starts polls the consumer
    /// inside a turn of the store, as [`StreamConsumer`] states. A
    /// guest's write that started before the pipe is served in the
    /// next turn. A stream whose writer already dropped its end ends
    /// here: the readable end drops, and the consumer is dropped
    /// without a poll.
    ///
    /// A stream the host created with [`new`](Self::new) and pipes to
    /// itself involves no guest: a host task of the store copies from
    /// the producer to the consumer inside turns, until the producer
    /// is done and the consumer has taken all it delivered, or the
    /// consumer answers [`StreamResult::Dropped`], and then drops
    /// both. The items cross as the values they are, so a failure of
    /// either side reaches whichever driver of the store is running.
    ///
    /// `D` is the store's host data. [`Store::as_context_mut`] reaches
    /// the context from a store the host holds, and a host `async`
    /// function reaches it through [`Accessor::with`].
    ///
    /// Fails with the not-present cause of [`CopyCause`] when the
    /// store holds no readable end under this reader: the end is gone,
    /// or the reader came from a value that was closed. Wasmtime fails
    /// its pipe there too. Fails with the not-held cause when the end
    /// is there but the host gave it up, through another value that
    /// names it: it was lowered into a guest or closed, or piped while
    /// a write of it is in flight or piped to itself by a stream the
    /// host created. Wasmtime lets those pipes through, as
    /// [`CopyCause::NotHeldByHost`] states. An end another value piped
    /// already, while a guest holds the writable end and no write of
    /// it is in flight, takes this consumer in place of that one,
    /// which is dropped unpolled, as Wasmtime's pipe replaces it.
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn pipe<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
        consumer: impl StreamConsumer<D, Item = T>,
    ) -> Result<()> {
        let consumer = ConsumerEnd {
            consumer: Box::pin(consumer),
            dropped: false,
            data: PhantomData,
        };
        pipe_readable_end(store, self.end, EndKind::StreamReadable, consumer)
    }

    /// Convert the reader into a [`StreamAny`], the untyped value of
    /// its end, in the store `store` reaches. The name is Wasmtime's,
    /// and [`StreamAny::try_from_stream_reader`] states the contract.
    pub fn try_into_stream_any<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
    ) -> Result<StreamAny> {
        StreamAny::try_from_stream_reader(store, self)
    }

    /// Convert `stream` into a reader whose items are of the Rust type
    /// `T`, after checking that the stream carries the type `T`
    /// projects to. The name is Wasmtime's, and
    /// [`StreamAny::try_into_stream_reader`] states the contract.
    pub fn try_from_stream_any(stream: StreamAny) -> Result<Self> {
        stream.try_into_stream_reader()
    }
}

impl<T> StreamReader<T> {
    /// Close this stream in the store `store` reaches: drop its
    /// readable end. The name and the contract are Wasmtime's.
    ///
    /// A write in progress on the writable end completes with the
    /// dropped result, and a later write sees that result at once. A
    /// stream the host created with [`new`](Self::new) takes its
    /// producer with it, dropped unpolled, because nobody is left to
    /// read what it would produce.
    ///
    /// `D` is the store's host data. [`Store::as_context_mut`] reaches
    /// the context from a store the host holds.
    /// [`close_with`](Self::close_with) closes the stream from inside
    /// a poll, through an accessor.
    ///
    /// Fails with the not-present cause of [`CopyCause`] when the
    /// store holds no readable end under this reader: the reader was
    /// closed already, or its end is gone. Wasmtime fails those closes
    /// too. Fails with the not-held cause when the end lives on in a
    /// guest, lowered through another value that names it, or with a
    /// consumer another value piped it to that serves a write in
    /// flight or that the pipe of a stream the host created holds,
    /// which Wasmtime lets through, as [`CopyCause::NotHeldByHost`]
    /// states. A close of an end another value piped to a consumer,
    /// while a guest holds the writable end and no write of it is in
    /// flight, drops the end and the consumer, unpolled, and the
    /// writer sees the dropped result, as Wasmtime's close does. A
    /// close of an end that
    /// another value closed already, while a guest still holds the
    /// writable end, succeeds and does nothing, as Wasmtime's does.
    /// The reader names no end after a close, whether it succeeds or
    /// fails, as Wasmtime's names none.
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    pub fn close<D: 'static>(&mut self, store: &mut StoreContext<'_, D>) -> Result<()> {
        close_readable_end(store, &mut self.end, EndKind::StreamReadable)
    }

    /// Close this stream through `accessor`, as [`close`](Self::close)
    /// closes it through a store context. The name is Wasmtime's.
    ///
    /// Fails with the store-not-in-poll cause when no poll of the
    /// accessor's store is running, and with the recursive-driver
    /// cause from inside another reach of the same store, as
    /// [`Accessor::with`] states.
    pub fn close_with<D: 'static>(&mut self, accessor: &Accessor<D>) -> Result<()> {
        accessor.with(|store| self.close(store))?
    }

    /// Pair this reader with `accessor`, into a guard that closes the
    /// stream when it drops. The name is Wasmtime's.
    ///
    /// The guard closes through the accessor, so it reaches the store
    /// only when it drops inside a poll of that store, as
    /// [`GuardedStreamReader`] states.
    pub fn guard<D: 'static>(self, accessor: Accessor<D>) -> GuardedStreamReader<T, D> {
        GuardedStreamReader::new(accessor, self)
    }
}

impl<T> StreamReaderInternal for StreamReader<T> {
    fn from_end(end: EndId) -> Self {
        Self {
            end,
            item: PhantomData,
        }
    }

    fn end(&self) -> EndId {
        self.end
    }
}

/// The argument slot a delivery's failure is labelled with: the
/// pointer of the read the items are lowered into, which is the
/// second argument of `stream.read`.
const POINTER_ARGUMENT: AbiPosition = AbiPosition::Argument(1);

/// The writable end of a stream the host serves through `P`, with
/// the items `P` delivered that the reader has not taken yet.
struct ProducerEnd<D: 'static, P: StreamProducer<D>> {
    producer: Pin<Box<P>>,
    /// The items waiting for the reader, in order. This is the vector
    /// the producer's destination holds, so a producer that takes it
    /// back gets the allocation of its earlier poll.
    waiting: Vec<P::Item>,
    /// Whether the producer answered that the stream is over.
    dropped: bool,
    data: PhantomData<fn() -> D>,
}

impl<D: 'static, P> HostWriter<D> for ProducerEnd<D, P>
where
    P: StreamProducer<D>,
    P::Item: ComponentValue,
{
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        remaining: Option<usize>,
        finish: bool,
    ) -> Poll<Result<()>> {
        if !self.waiting.is_empty() || self.dropped {
            return Poll::Ready(Ok(()));
        }
        let destination = Destination::new(&mut self.waiting, remaining);
        let answer = match self
            .producer
            .as_mut()
            .poll_produce(cx, store, destination, finish)
        {
            Poll::Pending if self.waiting.is_empty() => return Poll::Pending,
            Poll::Pending => return Poll::Ready(Err(broke(CopyCause::ProducerPendingAfterItems))),
            Poll::Ready(answer) => answer,
        };
        Poll::Ready(match answer {
            Err(error) => Err(error),
            // A reader that is the host asks for items as a guest's
            // read of some does.
            Ok(StreamResult::Completed) if self.waiting.is_empty() && remaining != Some(0) => {
                Err(broke(CopyCause::ProducerCompletedWithoutItems))
            }
            Ok(StreamResult::Completed) => Ok(()),
            Ok(StreamResult::Cancelled) if !finish => {
                Err(broke(CopyCause::ProducerCancelledWithoutFinish))
            }
            // Items stored with a cancelled answer are kept and reach
            // the reader, as Wasmtime's code keeps them, although its
            // documentation says such an answer traps.
            Ok(StreamResult::Cancelled) => Ok(()),
            Ok(StreamResult::Dropped) => {
                self.dropped = true;
                Ok(())
            }
        })
    }

    fn waiting(&self) -> usize {
        self.waiting.len()
    }

    fn finished(&self) -> bool {
        self.dropped
    }

    fn deliver(
        &mut self,
        cx: &mut BoundaryContext<'_, StoreData<D>>,
        offset: usize,
        ty: &ValueType,
        count: usize,
    ) -> Result<()> {
        let size = size_of(ty);
        let count = count.min(self.waiting.len());
        for (i, item) in self.waiting.drain(..count).enumerate() {
            item.store(cx, offset + i * size, ty, POINTER_ARGUMENT)?;
        }
        Ok(())
    }

    fn take_items(&mut self) -> Vec<Val> {
        self.waiting.drain(..).map(ComponentValue::to_val).collect()
    }
}

/// The failure of a poll whose answer the producer or consumer
/// contract forbids.
fn broke(cause: CopyCause) -> Error {
    Error::Copy(cause)
}

/// The readable end of a stream the host serves through `C`.
struct ConsumerEnd<D: 'static, C: StreamConsumer<D>> {
    consumer: Pin<Box<C>>,
    /// Whether the consumer answered that it takes nothing more.
    dropped: bool,
    data: PhantomData<fn() -> D>,
}

impl<D: 'static, C> HostConsumer<D> for ConsumerEnd<D, C>
where
    C: StreamConsumer<D>,
    C::Item: ComponentValue,
{
    type Item = C::Item;

    fn consume(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        mut source: Source<'_, C::Item>,
        finish: bool,
        earlier: usize,
    ) -> Poll<Result<()>> {
        if self.dropped {
            return Poll::Ready(Ok(()));
        }
        let offered = source.remaining();
        let poll = self
            .consumer
            .as_mut()
            .poll_consume(cx, store, source.reborrow(), finish);
        // A consumer may take items and answer pending: that is the
        // backpressure of the contract, and the items count once a
        // later poll answers ready.
        let Poll::Ready(answer) = poll else {
            return Poll::Pending;
        };
        let taken = earlier + (offered - source.remaining());
        Poll::Ready(match answer {
            Err(error) => Err(error),
            Ok(StreamResult::Completed) if taken == 0 && offered > 0 => {
                Err(broke(CopyCause::ConsumerCompletedWithoutItems))
            }
            Ok(StreamResult::Completed) => Ok(()),
            Ok(StreamResult::Cancelled) if !finish => {
                Err(broke(CopyCause::ConsumerCancelledWithoutFinish))
            }
            // Items taken by a poll that answers cancelled count as the
            // write's progress, as Wasmtime's code counts them.
            Ok(StreamResult::Cancelled) => Ok(()),
            Ok(StreamResult::Dropped) => {
                self.dropped = true;
                Ok(())
            }
        })
    }

    fn finished(&self) -> bool {
        self.dropped
    }
}
