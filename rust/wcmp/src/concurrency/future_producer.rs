//! The writing side a host gives a future it creates.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::error::{Error, Result};
use crate::store::StoreContext;

/// The writing side a host gives a future it creates. The name and
/// the contract are Wasmtime's.
///
/// [`FutureReader::new`](super::FutureReader::new) takes a producer
/// and returns the readable end of a future whose writable end is the
/// producer. When the reader starts its read, the store polls the
/// producer inside a turn, with the store context of the store that
/// owns the end and the `finish` flag. `D` is that store's host data.
///
/// The contract is the one [`StreamProducer`](super::StreamProducer)
/// states, narrowed to one value. A producer that has the value
/// answers `Some` with it, and the value reaches the reader. A
/// producer that has nothing ready stores the waker and answers
/// pending. `finish` is true when the guest cancelled its read, and
/// the producer may then answer `None` to say it delivered nothing.
/// The producer is not done then, and the next read polls it again.
/// Answering `None` when `finish` is false is a failure. A poll that
/// answers with an error fails the guest's built-in with that error.
///
/// Every future whose output is a `Result` is a producer, so a host
/// writes a future with an `async` block, as in Wasmtime.
///
/// The `Send` half of the bound is the one per-target line: required
/// natively, absent in the browser, for the reason the stream
/// producer states.
#[cfg(not(target_arch = "wasm32"))]
pub trait FutureProducer<D: 'static>: Send + 'static {
    /// The type of the value the future carries.
    type Item;

    /// Produce the future's value. See the trait's documentation for
    /// the contract.
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        finish: bool,
    ) -> Poll<Result<Option<Self::Item>>>;
}

/// The writing side a host gives a future it creates. See the native
/// definition for the contract and for why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait FutureProducer<D: 'static>: 'static {
    /// The type of the value the future carries.
    type Item;

    /// Produce the future's value. See the native definition for the
    /// contract.
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        finish: bool,
    ) -> Poll<Result<Option<Self::Item>>>;
}

/// A future whose output is a `Result` produces its `Ok` value, and
/// its error fails the read. A cancelled read of a future that is not
/// ready delivers nothing.
#[cfg(not(target_arch = "wasm32"))]
impl<D, Item, E, F> FutureProducer<D> for F
where
    D: 'static,
    E: Into<Error>,
    F: Future<Output = core::result::Result<Item, E>> + ?Sized + Send + 'static,
{
    type Item = Item;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, D>,
        finish: bool,
    ) -> Poll<Result<Option<Item>>> {
        produce(self, cx, finish)
    }
}

/// A future whose output is a `Result` produces its `Ok` value. See
/// the native implementation.
#[cfg(target_arch = "wasm32")]
impl<D, Item, E, F> FutureProducer<D> for F
where
    D: 'static,
    E: Into<Error>,
    F: Future<Output = core::result::Result<Item, E>> + ?Sized + 'static,
{
    type Item = Item;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, D>,
        finish: bool,
    ) -> Poll<Result<Option<Item>>> {
        produce(self, cx, finish)
    }
}

/// Poll `future` as a producer: its value is the future's value, its
/// error the read's failure, and a pending future asked to finish
/// delivers nothing.
fn produce<Item, E, F>(
    future: Pin<&mut F>,
    cx: &mut Context<'_>,
    finish: bool,
) -> Poll<Result<Option<Item>>>
where
    E: Into<Error>,
    F: Future<Output = core::result::Result<Item, E>> + ?Sized,
{
    match future.poll(cx) {
        Poll::Ready(Ok(value)) => Poll::Ready(Ok(Some(value))),
        Poll::Ready(Err(error)) => Poll::Ready(Err(error.into())),
        Poll::Pending if finish => Poll::Ready(Ok(None)),
        Poll::Pending => Poll::Pending,
    }
}
