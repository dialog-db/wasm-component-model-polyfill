//! The writing side a host gives a stream it creates.

use core::pin::Pin;
use core::task::{Context, Poll};

use crate::error::Result;
use crate::store::StoreContext;

use super::destination::Destination;
use super::stream_result::StreamResult;

/// The writing side a host gives a stream it creates. The name and
/// the contract are Wasmtime's.
///
/// [`StreamReader::new`](super::StreamReader::new) takes a producer
/// and returns the readable end of a stream whose writable end is the
/// producer. Whenever the reader starts a read, the store polls the
/// producer inside a turn, with the store context of the store that
/// owns the end, the [`Destination`] of the read, and the `finish`
/// flag. `D` is that store's host data.
///
/// A poll answers in one of these ways:
///
/// - A producer that has items stores them in the destination and
///   answers [`StreamResult::Completed`] when it can produce more, or
///   [`StreamResult::Dropped`] when it cannot. Items beyond the
///   reader's capacity stay with the end and satisfy later reads
///   before the producer is polled again. Answering `Completed` with
///   no item to a read that asked for some is a failure, because the
///   reader would be told of a read that moved nothing.
/// - A producer that has nothing ready stores the waker of `cx` and
///   answers pending, without storing an item. The store polls it
///   again once the waker is woken.
/// - A read of zero items reaches the producer as a destination
///   whose [`remaining`](Destination::remaining) is `Some(0)`. The
///   guest is asking whether the stream is ready. The producer can
///   answer `Completed` at once, or answer pending until it has items
///   and `Completed` then.
/// - `finish` is true when the guest cancelled its read. The producer
///   must then answer ready as soon as it can: with
///   [`StreamResult::Cancelled`] when it delivered nothing, and it
///   may answer pending once more to finish work it had started.
///   Answering `Cancelled` when `finish` is false is a failure.
///   Items stored in the destination by a poll that answers
///   `Cancelled` are not refused: they stay with the end and reach
///   the reader like any others. Wasmtime's documentation says such
///   an answer traps, but its code keeps the items, and the polyfill
///   follows the code.
/// - A poll that answers with an error fails the guest's built-in
///   with that error, as the failure of a host task fails the guest
///   task that called it: the built-in fails at once when the read
///   is polled before it returns, and the task that started the read
///   ends with the error when the failure comes in a later turn.
///
/// A `Vec<T>` of `Unpin` items, a `Box<[T]>`, and an
/// [`iter::Empty<T>`](core::iter::Empty) are producers, as in
/// Wasmtime: the first two deliver every item they hold on their
/// first poll and end the stream, and the reader takes the items over
/// as many reads as it needs; the empty iterator ends the stream at
/// once.
///
/// Parts of Wasmtime's trait are left out. There is no `Buffer`
/// type: a producer always delivers into the vector
/// [`Destination`] holds. There is no `try_into`, the hook through
/// which Wasmtime takes a producer back out of a stream the host
/// reads itself, because the host does not read a stream here. The
/// producers Wasmtime gives `futures::stream::Empty` and to the
/// `bytes` types are absent, because the polyfill depends on neither
/// crate.
///
/// The `Send` half of the bound is the one per-target line. It is
/// required natively, so that a store stays `Send`. It is absent in
/// the browser, so that a producer can await a JavaScript promise,
/// which is not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait StreamProducer<D: 'static>: Send + 'static {
    /// The type of each item the stream carries.
    type Item;

    /// Deliver items for the read `destination` describes. See the
    /// trait's documentation for the contract.
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        destination: Destination<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}

/// The writing side a host gives a stream it creates. See the native
/// definition for the contract and for why the `Send` half of the
/// bound is absent here.
#[cfg(target_arch = "wasm32")]
pub trait StreamProducer<D: 'static>: 'static {
    /// The type of each item the stream carries.
    type Item;

    /// Deliver items for the read `destination` describes. See the
    /// native definition for the contract.
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, D>,
        destination: Destination<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}

/// Implement [`StreamProducer`] for a type that holds all of its items
/// at once, on each target: natively with the items bound `Send`, as
/// the trait's `Send` bound asks, and in the browser without. `$bound`
/// is a further bound on the items, and `$take` turns the producer
/// into the vector of its items, leaving it empty.
macro_rules! whole_producer {
    ($(#[$doc:meta])* $ty:ty $(where T: $bound:path)?, $take:expr) => {
        $(#[$doc])*
        #[cfg(not(target_arch = "wasm32"))]
        impl<D: 'static, T: $($bound +)? Send + 'static> StreamProducer<D> for $ty {
            type Item = T;

            fn poll_produce(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _store: &mut StoreContext<'_, D>,
                destination: Destination<'_, T>,
                _finish: bool,
            ) -> Poll<Result<StreamResult>> {
                deliver_all(destination, $take(self.get_mut()))
            }
        }

        $(#[$doc])*
        #[cfg(target_arch = "wasm32")]
        impl<D: 'static, T: $($bound +)? 'static> StreamProducer<D> for $ty {
            type Item = T;

            fn poll_produce(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _store: &mut StoreContext<'_, D>,
                destination: Destination<'_, T>,
                _finish: bool,
            ) -> Poll<Result<StreamResult>> {
                deliver_all(destination, $take(self.get_mut()))
            }
        }
    };
}

whole_producer!(
    /// A vector delivers all of its items on its first poll and ends
    /// the stream.
    Vec<T> where T: Unpin,
    |items: &mut Vec<T>| core::mem::take(items)
);

whole_producer!(
    /// A boxed slice delivers all of its items on its first poll and
    /// ends the stream.
    Box<[T]>,
    |items: &mut Box<[T]>| core::mem::take(items).into_vec()
);

whole_producer!(
    /// An empty iterator ends the stream on its first poll.
    core::iter::Empty<T>,
    |_: &mut core::iter::Empty<T>| Vec::new()
);

/// Store `items` in `destination` and end the stream. The items the
/// reader cannot take at once stay with the end for its later reads.
fn deliver_all<T>(
    mut destination: Destination<'_, T>,
    items: Vec<T>,
) -> Poll<Result<StreamResult>> {
    if !items.is_empty() {
        destination.set_buffer(items);
    }
    Poll::Ready(Ok(StreamResult::Dropped))
}
