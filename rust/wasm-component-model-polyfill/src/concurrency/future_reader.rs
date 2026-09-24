//! The readable end of a future the host holds.

use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::error::{AbiPosition, CopyCause, Error, Result};
use crate::internal::FutureReaderInternal;
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt, StoreData};
use crate::types::ValueType;

use super::end_id::EndId;
use super::end_kind::EndKind;
use super::future_producer::FutureProducer;
use super::host_writer::HostWriter;

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
/// A reader belongs to the store it was created in, and is lowered
/// only into a guest of that store. It names its end by an index and
/// a generation and carries no identity of the store, so the lower
/// cannot tell a reader from another store. Using a reader with a
/// store other than the one that made it is a host error the
/// polyfill does not detect, as with a
/// [`ResourceHandle`](crate::ResourceHandle): when the other store
/// holds an unlowered readable end under the same index and
/// generation, that end is the one lowered, and the guest reads the
/// other store's future. Wasmtime's reader carries no store identity
/// either. The reader is moved when it is lowered, so an end crosses
/// into a guest once.
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
        _remaining: usize,
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
}
