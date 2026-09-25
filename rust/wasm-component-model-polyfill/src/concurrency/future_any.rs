//! The readable end of a future, as an untyped value carries it.

use crate::error::{CopyCause, Error, Result};
use crate::executor::close_readable_end;
use crate::internal::{FutureAnyInternal, FutureReaderInternal};
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt};
use crate::types::ValueType;

use super::end_id::EndId;
use super::end_kind::EndKind;
use super::future_reader::FutureReader;

/// The readable end of a future, as [`Val::Future`](crate::Val::Future)
/// carries it. The name is Wasmtime's.
///
/// It holds the end and the type of the value the future carries,
/// so it is how a host that does not name the payload type in Rust
/// holds a future: the untyped entries of the linker receive one for
/// a future parameter, and an untyped call returns one for a future
/// result. The lift takes the guest's entry away, and the host holds
/// the end. Lowering one into a guest, as an argument or as a result,
/// checks that the guest's type carries the same payload and enters
/// the end in the guest's handle table.
///
/// A host that knows the payload type converts the value into a
/// [`FutureReader`] with [`try_into_future_reader`] to read the
/// future, and a reader converts back with
/// [`FutureReader::try_into_future_any`]. A host that wants none of
/// the future closes it with [`close`]. The polyfill offers no read
/// or write that does not name the type, as Wasmtime does not.
///
/// The lifecycle rule is Wasmtime's: a future the host holds must be
/// lowered into a guest, piped through its typed reader, or closed.
/// The value has no `Drop` of its own, because its end lives in the
/// store, which the value cannot reach, so a value dropped otherwise
/// leaks its end until the store drops, and a guest that writes to
/// the future waits for good.
///
/// The value names its end by an index and a generation, and cloning
/// it copies the name, not the end, as cloning Wasmtime's value
/// copies its id. Once one copy lowers, pipes, or closes the end, the
/// others are refused those uses, as [`CopyCause::NotHeldByHost`]
/// states, except that a close of an end closed already succeeds and
/// does nothing, that an end another copy piped to a consumer while a
/// guest holds the writable end may be piped again or closed while no
/// write of it is in flight, and that each copy still converts into a
/// reader. The copy that closed names no end afterwards. The value
/// belongs to the store that lifted or created its end, and carries no
/// identity of the store, as a [`FutureReader`] carries none: using
/// it with another store is a host error the polyfill does not
/// detect, as the reader states.
///
/// [`try_into_future_reader`]: Self::try_into_future_reader
/// [`close`]: Self::close
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FutureAny {
    end: EndId,
    /// Boxed, so that a `Val` that carries one stays the size it is
    /// without: the copy budget charges that size per element.
    payload: Option<Box<ValueType>>,
}

impl FutureAny {
    /// Close the future, in the store `store` reaches. The name and
    /// the contract are Wasmtime's.
    ///
    /// The readable end drops, as a guest's `future.drop-readable`
    /// drops it. A guest's write that is pending completes with the
    /// dropped result, and a later write reports the dropped result at
    /// once. A future the host created drops its producer with the
    /// end, because nothing is left to read what it would produce.
    ///
    /// `D` is the store's host data. [`Store::as_context_mut`] reaches
    /// the context from a store the host holds, and a host `async`
    /// function reaches it through [`Accessor::with`].
    ///
    /// Fails with the not-present or the not-held cause of
    /// [`CopyCause`], or does nothing, as
    /// [`FutureReader::close`](super::FutureReader::close) states for
    /// a reader: a second close of this value fails, and a close of
    /// an end that a clone closed already does nothing.
    ///
    /// [`Store::as_context_mut`]: crate::Store::as_context_mut
    /// [`Accessor::with`]: crate::Accessor::with
    pub fn close<D: 'static>(&mut self, store: &mut StoreContext<'_, D>) -> Result<()> {
        close_readable_end(store, &mut self.end, EndKind::FutureReadable)
    }

    /// Convert the value into a [`FutureReader<T>`], whose value is
    /// of the Rust type `T`. The name is Wasmtime's.
    ///
    /// Fails with the payload-mismatch cause of [`CopyCause`] when the
    /// future does not carry the type `T` projects to, as the lift of
    /// a typed reader fails. A future with no payload converts into no
    /// reader, because no Rust type projects to the absence of one.
    pub fn try_into_future_reader<T: ComponentValue>(self) -> Result<FutureReader<T>> {
        if self.payload.as_deref() != Some(&T::value_type()) {
            return Err(Error::Copy(CopyCause::PayloadMismatch {
                kind: EndKind::FutureReadable,
            }));
        }
        Ok(FutureReader::from_end(self.end))
    }

    /// Convert `reader`, a readable end the host holds in the store
    /// `store` reaches, into an untyped value that carries the
    /// future's payload type. The name is Wasmtime's.
    ///
    /// The conversion checks only that the end is in the store,
    /// whoever holds it, as Wasmtime's does, so it succeeds for a
    /// reader whose end another value lowered, piped, or closed, and
    /// the value it answers may use the end only as
    /// [`CopyCause::NotHeldByHost`] states.
    ///
    /// Fails with the not-present cause of [`CopyCause`] when the
    /// store holds no readable end under `reader`: the reader came
    /// from a value that was closed, or its end is gone. Wasmtime
    /// fails such a reader with the same message.
    pub fn try_from_future_reader<T, D: 'static>(
        store: &mut StoreContext<'_, D>,
        reader: FutureReader<T>,
    ) -> Result<Self> {
        let payload = store
            .internal()
            .lock_tables()?
            .tasks
            .readable_payload(reader.end(), EndKind::FutureReadable)?;
        Ok(Self::new(reader.end(), payload))
    }
}

impl FutureAnyInternal for FutureAny {
    fn new(end: EndId, payload: Option<ValueType>) -> Self {
        Self {
            end,
            payload: payload.map(Box::new),
        }
    }

    fn end(&self) -> EndId {
        self.end
    }

    fn payload(&self) -> Option<&ValueType> {
        self.payload.as_deref()
    }
}
