//! The readable end of a stream the host holds.

use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::abi::layout::size_of;
use crate::error::{AbiPosition, CopyCause, Error, Result};
use crate::internal::{DestinationInternal, StreamReaderInternal};
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt, StoreData};
use crate::types::ValueType;

use super::destination::Destination;
use super::end_id::EndId;
use super::end_kind::EndKind;
use super::host_writer::HostWriter;
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
/// There is no writer type. A host writes a stream by supplying a
/// producer when it creates the stream.
///
/// A reader belongs to the store it was created in, and is lowered
/// only into a guest of that store. It names its end by an index and
/// a generation and carries no identity of the store, so the lower
/// cannot tell a reader from another store. Using a reader with a
/// store other than the one that made it is a host error the
/// polyfill does not detect, as with a
/// [`ResourceHandle`](crate::ResourceHandle): when the other store
/// holds an unlowered readable end under the same index and
/// generation, that end is the one lowered, and the guest reads the
/// other store's stream. Wasmtime's reader carries no store identity
/// either. The reader is moved when it is lowered, so an end crosses
/// into a guest once.
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
            .insert_host_ends(Some(T::value_type()), EndKind::StreamWritable);
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
        remaining: usize,
        finish: bool,
    ) -> Poll<Result<()>> {
        if !self.waiting.is_empty() || self.dropped {
            return Poll::Ready(Ok(()));
        }
        let destination = Destination::new(&mut self.waiting, Some(remaining));
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
            Ok(StreamResult::Completed) if self.waiting.is_empty() && remaining > 0 => {
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
}

/// The failure of a poll whose answer the producer contract forbids.
fn broke(cause: CopyCause) -> Error {
    Error::Copy(cause)
}
