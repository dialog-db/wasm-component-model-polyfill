//! The buffer of a write a host consumer serves.

use wasm_runtime_layer::AsContextMut;

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::size_of;
use crate::error::{AbiPosition, Error, Result};
use crate::internal::{ErrorInternal, SourceInternal};
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt};

use super::copy_buffer::CopyBuffer;

/// The argument slot a read's failure is labelled with: the pointer
/// of the write the items are lifted from, which is the second
/// argument of `stream.write` and `future.write`.
const POINTER_ARGUMENT: AbiPosition = AbiPosition::Argument(1);

/// The buffer of one write a host
/// [`StreamConsumer`](super::StreamConsumer) or
/// [`FutureConsumer`](super::FutureConsumer) serves.
///
/// The name is Wasmtime's. The view is the polyfill's own, over a
/// `Vec<T>`: [`read`](Self::read) moves items into a vector the
/// consumer holds. Wasmtime's source also reads into buffer types of
/// its own and offers a direct view into the guest's memory for a
/// stream of bytes. The runtime layer the polyfill stands on offers
/// only a copy out of a memory, so every item a consumer takes is a
/// copy on the host. This type is where a direct view lands if the
/// layer ever allows one.
///
/// When the writer is a guest, the items stay in the guest's memory
/// until the consumer reads them: [`read`](Self::read) lifts each one
/// then, through the boundary context of the write, so a payload that
/// carries an owned resource moves the handle out of the writer's
/// table only when the consumer takes the item. The items the
/// consumer does not take stay with the writer, and the write
/// reports only the items taken. A payload of a number type lifts
/// value by value too: the byte copy a number payload takes between
/// two guests builds no host value, and a consumer needs one, as
/// Wasmtime's `read` lifts typed values whatever the payload. When
/// the writer is a host producer, the items are the ones it
/// delivered, already on the host.
pub struct Source<'a, T> {
    inner: Inner<'a, T>,
}

/// Where the items of a [`Source`] come from.
enum Inner<'a, T> {
    /// A host producer's items, taken from the front.
    Host(&'a mut Vec<T>),
    /// A guest's write: its buffer as it stood when the poll began,
    /// and the count of items this poll has taken from it so far.
    Guest {
        write: &'a CopyBuffer,
        taken: &'a mut u32,
    },
}

impl<T> Source<'_, T> {
    /// How many items the writer still offers.
    ///
    /// The count can be zero. A guest that writes zero items is asking
    /// whether the stream is ready to be written, which the
    /// Concurrency explainer calls stream readiness. The consumer can
    /// answer [`StreamResult::Completed`](super::StreamResult::Completed)
    /// at once, or wait until it can take items and answer then.
    ///
    /// Wasmtime's `remaining` takes the store as well; this one reads
    /// what the source recorded when the poll began.
    pub fn remaining(&self) -> usize {
        match &self.inner {
            Inner::Host(items) => items.len(),
            Inner::Guest { write, taken } => (write.remain() - **taken) as usize,
        }
    }

    /// Move up to `count` of the items the writer offers, in order,
    /// onto the end of `buffer`. Fewer move when fewer remain.
    ///
    /// `store` is the store context the poll was handed, which a read
    /// from a guest lifts the items through. A lift that fails, for
    /// example because a string in the guest's memory is not valid
    /// UTF-8, fails the read, and the items lifted before it stay in
    /// `buffer`. Wasmtime's `read` takes the store too, and fills the
    /// spare capacity of its buffer rather than a count.
    pub fn read<D: 'static>(
        &mut self,
        store: &mut StoreContext<'_, D>,
        buffer: &mut Vec<T>,
        count: usize,
    ) -> Result<()>
    where
        T: ComponentValue,
    {
        let count = count.min(self.remaining());
        if count == 0 {
            return Ok(());
        }
        match &mut self.inner {
            Inner::Host(items) => {
                buffer.extend(items.drain(..count));
                Ok(())
            }
            Inner::Guest { write, taken } => lift_items(store, write, taken, buffer, count),
        }
    }

    /// Borrow the source again, for a consumer that hands it to
    /// another one it wraps.
    pub fn reborrow(&mut self) -> Source<'_, T> {
        let inner = match &mut self.inner {
            Inner::Host(items) => Inner::Host(&mut **items),
            Inner::Guest { write, taken } => Inner::Guest { write, taken },
        };
        Source { inner }
    }
}

/// Lift `count` items of the guest's write `write`, from the first
/// one no poll has taken yet, onto the end of `buffer`, counting each
/// in `taken` as it lands.
fn lift_items<D: 'static, T: ComponentValue>(
    store: &mut StoreContext<'_, D>,
    write: &CopyBuffer,
    taken: &mut u32,
    buffer: &mut Vec<T>,
    count: usize,
) -> Result<()> {
    let ty = write
        .payload
        .as_ref()
        .ok_or_else(|| Error::internal("a typed source read a stream that carries no values"))?;
    let tables = store.internal().tables_handle();
    let (options, instance) = BoundaryInstance::resolve(&write.options, &write.abi_state, &tables)?;
    let mut cx = BoundaryContext::new(
        store.internal().runtime_mut().as_context_mut(),
        options,
        instance,
        None,
    );
    let size = size_of(ty);
    buffer.reserve(count);
    for _ in 0..count {
        let offset = write.pointer as usize + (write.progress + *taken) as usize * size;
        buffer.push(T::load(&mut cx, offset, ty, POINTER_ARGUMENT)?);
        *taken += 1;
    }
    Ok(())
}

impl<'a, T> SourceInternal<'a, T> for Source<'a, T> {
    fn host(items: &'a mut Vec<T>) -> Self {
        Self {
            inner: Inner::Host(items),
        }
    }

    fn guest(write: &'a CopyBuffer, taken: &'a mut u32) -> Self {
        Self {
            inner: Inner::Guest { write, taken },
        }
    }
}
