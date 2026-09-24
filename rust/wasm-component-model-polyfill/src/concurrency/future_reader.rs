//! The readable end of a future the host holds.

use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::error::{AbiPosition, CopyCause, Error, Result};
use crate::executor::{close_readable_end, pipe_readable_end};
use crate::internal::FutureReaderInternal;
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt, StoreData};
use crate::types::ValueType;
use crate::value::Val;

use super::accessor::Accessor;
use super::end_id::EndId;
use super::end_kind::EndKind;
use super::future_consumer::FutureConsumer;
use super::future_producer::FutureProducer;
use super::guarded_future_reader::GuardedFutureReader;
use super::host_consumer::HostConsumer;
use super::host_writer::HostWriter;
use super::source::Source;

/// The readable end of a `future<T>` the host holds. The name is
/// Wasmtime's.
///
/// A host creates a future with [`FutureReader::new`], giving it the
/// producer that writes the one value, and hands the reader to a
/// guest: as the result of a typed host function, or as an argument
/// of a typed call. The reader implements
/// [`ComponentValue`](crate::ComponentValue) for that, and lowering
/// it into a guest enters a readable end in the guest's handle table
/// that shares the future with the producer. The guest's
/// `future.read` then polls the producer inside a turn of the store.
///
/// Every future whose output is a `Result` is a
/// [`FutureProducer`], so a host writes a future with an `async`
/// block.
///
/// A host receives a reader the other way: a future in the result of
/// a typed call, or in a parameter of a typed host function, arrives
/// as one, and the guest's entry for the end is gone. The lift checks
/// that the guest's future carries the type `T` projects to. The host
/// then reads the value with [`pipe`](Self::pipe), which makes a
/// [`FutureConsumer`] its reading side, and the guest's
/// `future.write` polls the consumer inside a turn of the store.
///
/// The lifecycle rule is Wasmtime's: a reader the host holds must be
/// lowered into a guest, piped, or closed with [`close`](Self::close).
/// A reader dropped otherwise leaks its end until the store drops,
/// and a guest that writes to the future waits for good.
/// [`guard`](Self::guard) pairs a reader with an accessor into a
/// [`GuardedFutureReader`], which closes the future when it drops
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
/// store's future. The close needs no identity of the store to keep
/// its own contract, so none is added. Wasmtime's
/// reader carries no store identity either. The reader is moved when
/// it is lowered or piped, so an end crosses into a guest, or reaches
/// a consumer, once.
pub struct FutureReader<T> {
    end: EndId,
    item: PhantomData<fn() -> T>,
}

impl<T: ComponentValue> FutureReader<T> {
    /// Create a future in the store `store` reaches, whose writable
    /// end is `producer`, and return its readable end.
    ///
    /// The future carries a value of the type `T` projects to. `D` is
    /// the store's host data. [`Store::as_context_mut`] reaches the
    /// context from a store the host holds, and a host `async`
    /// function reaches it through [`Accessor::with`].
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn new<D: 'static>(
        store: &mut StoreContext<'_, D>,
        producer: impl FutureProducer<D, Item = T>,
    ) -> Result<Self> {
        let (readable, writable) = store
            .internal()
            .lock_tables()?
            .tasks
            .insert_host_ends(Some(T::value_type()), EndKind::FutureWritable);
        store.internal().scheduler_mut().insert_host_writer(
            writable,
            Box::new(ProducerEnd {
                producer: Box::pin(producer),
                value: None,
                produced: false,
                data: PhantomData,
            }),
        );
        Ok(Self::from_end(readable))
    }

    /// Make `consumer` the reading side of this future, in the store
    /// `store` reaches, and give the reader up. The name and the
    /// contract are Wasmtime's.
    ///
    /// From then on the writer's write polls the consumer inside a
    /// turn of the store, as [`FutureConsumer`] states. A guest's
    /// write that started before the pipe is served in the next turn.
    /// A future whose writer already dropped its end ends here: the
    /// readable end drops, and the consumer is dropped without a poll.
    ///
    /// A future the host created with [`new`](Self::new) and pipes to
    /// itself involves no guest: a host task of the store polls the
    /// producer and hands its value to the consumer inside turns, and
    /// then drops both.
    ///
    /// Wasmtime asks the consumer to be `Unpin`, because it wraps the
    /// consumer as a stream's; the polyfill pins the consumer in a
    /// box of its own and asks nothing more.
    ///
    /// `D` is the store's host data. [`Store::as_context_mut`] reaches
    /// the context from a store the host holds, and a host `async`
    /// function reaches it through [`Accessor::with`].
    ///
    /// Fails with the not-held cause of [`CopyCause`] when the store
    /// holds no readable end for the host under this reader, as
    /// [`StreamReader::pipe`](super::StreamReader::pipe) states.
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn pipe<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
        consumer: impl FutureConsumer<D, Item = T>,
    ) -> Result<()> {
        let consumer = ConsumerEnd {
            consumer: Box::pin(consumer),
            received: false,
            data: PhantomData,
        };
        pipe_readable_end(store, self.end, EndKind::FutureReadable, consumer)
    }
}

impl<T> FutureReader<T> {
    /// Close this future in the store `store` reaches: drop its
    /// readable end. The name and the contract are Wasmtime's.
    ///
    /// A write in progress on the writable end completes with the
    /// dropped result, and a later write sees that result at once. A
    /// future the host created with [`new`](Self::new) takes its
    /// producer with it, dropped unpolled, because nobody is left to
    /// read the value it would produce.
    ///
    /// `D` is the store's host data. [`Store::as_context_mut`] reaches
    /// the context from a store the host holds.
    /// [`close_with`](Self::close_with) closes the future from inside
    /// a poll, through an accessor.
    ///
    /// Fails with the not-held cause of [`CopyCause`] when the store
    /// holds no readable end for the host under this reader, as
    /// [`StreamReader::close`](super::StreamReader::close) states.
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    pub fn close<D: 'static>(&mut self, store: &mut StoreContext<'_, D>) -> Result<()> {
        close_readable_end(store, self.end, EndKind::FutureReadable)
    }

    /// Close this future through `accessor`, as
    /// [`close`](Self::close) closes it through a store context. The
    /// name is Wasmtime's.
    ///
    /// Fails with the store-not-in-poll cause when no poll of the
    /// accessor's store is running, and with the recursive-driver
    /// cause from inside another reach of the same store, as
    /// [`Accessor::with`] states.
    pub fn close_with<D: 'static>(&mut self, accessor: &Accessor<D>) -> Result<()> {
        accessor.with(|store| self.close(store))?
    }

    /// Pair this reader with `accessor`, into a guard that closes the
    /// future when it drops. The name is Wasmtime's.
    ///
    /// The guard closes through the accessor, so it reaches the store
    /// only when it drops inside a poll of that store, as
    /// [`GuardedFutureReader`] states.
    pub fn guard<D: 'static>(self, accessor: Accessor<D>) -> GuardedFutureReader<T, D> {
        GuardedFutureReader::new(accessor, self)
    }
}

impl<T> FutureReaderInternal for FutureReader<T> {
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
/// pointer of the read the value is lowered into, which is the second
/// argument of `future.read`.
const POINTER_ARGUMENT: AbiPosition = AbiPosition::Argument(1);

/// The writable end of a future the host serves through `P`, with the
/// value `P` produced until the reader takes it.
struct ProducerEnd<D: 'static, P: FutureProducer<D>> {
    producer: Pin<Box<P>>,
    value: Option<P::Item>,
    /// Whether the producer has answered with its value. A future is
    /// written once, so the producer is never polled again after
    /// that. A producer that answered a cancelled read with nothing
    /// has not produced, and the next read polls it again.
    produced: bool,
    data: PhantomData<fn() -> D>,
}

impl<D: 'static, P> HostWriter<D> for ProducerEnd<D, P>
where
    P: FutureProducer<D>,
    P::Item: ComponentValue,
{
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        _remaining: Option<usize>,
        finish: bool,
    ) -> Poll<Result<()>> {
        if self.produced {
            return Poll::Ready(Ok(()));
        }
        let answer = match self.producer.as_mut().poll_produce(cx, store, finish) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(answer) => answer,
        };
        Poll::Ready(match answer {
            Err(error) => Err(error),
            Ok(Some(value)) => {
                self.value = Some(value);
                self.produced = true;
                Ok(())
            }
            Ok(None) if !finish => Err(Error::Copy(CopyCause::ProducerCancelledWithoutFinish)),
            // The read was cancelled before the value was ready. The
            // producer is not done: the next read polls it again, as
            // Wasmtime's future end answers cancelled and polls again.
            Ok(None) => Ok(()),
        })
    }

    fn waiting(&self) -> usize {
        usize::from(self.value.is_some())
    }

    fn finished(&self) -> bool {
        self.produced
    }

    fn deliver(
        &mut self,
        cx: &mut BoundaryContext<'_, StoreData<D>>,
        offset: usize,
        ty: &ValueType,
        count: usize,
    ) -> Result<()> {
        match self.value.take() {
            Some(value) if count > 0 => value.store(cx, offset, ty, POINTER_ARGUMENT),
            value => {
                self.value = value;
                Ok(())
            }
        }
    }

    fn take_items(&mut self) -> Vec<Val> {
        self.value
            .take()
            .map(ComponentValue::to_val)
            .into_iter()
            .collect()
    }
}

/// The readable end of a future the host serves through `C`.
struct ConsumerEnd<D: 'static, C: FutureConsumer<D>> {
    consumer: Pin<Box<C>>,
    /// Whether the consumer has taken the value. A future is read
    /// once, so the consumer is never polled again after that.
    received: bool,
    data: PhantomData<fn() -> D>,
}

impl<D: 'static, C> HostConsumer<D> for ConsumerEnd<D, C>
where
    C: FutureConsumer<D>,
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
        if self.received {
            return Poll::Ready(Ok(()));
        }
        let offered = source.remaining();
        let poll = self
            .consumer
            .as_mut()
            .poll_consume(cx, store, source.reborrow(), finish);
        // A consumer may take the value and answer pending, which
        // delays the write's completion until a later poll is ready.
        let Poll::Ready(answer) = poll else {
            return Poll::Pending;
        };
        answer?;
        let taken = earlier + (offered - source.remaining());
        Poll::Ready(if taken > 0 {
            self.received = true;
            Ok(())
        } else if finish {
            // The write was cancelled before the consumer took the
            // value, which stays with the writer: the write ends
            // cancelled, and the next write polls the consumer again.
            Ok(())
        } else {
            Err(Error::Copy(CopyCause::ConsumerCancelledWithoutFinish))
        })
    }

    fn finished(&self) -> bool {
        self.received
    }
}
