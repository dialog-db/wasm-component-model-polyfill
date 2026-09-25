//! The host's close of a readable end.
//!
//! A host that holds a stream or a future it will not read closes it,
//! through a typed reader or an untyped value, and both reach
//! [`close_readable_end`]. The close drops the readable end as a
//! guest's `stream.drop-readable` or `future.drop-readable` drops
//! one. A write in progress on the writable end completes with the
//! dropped result and the progress it made, and an idle writable end
//! is given the dropped result, so its next write reports it at once.
//! When the host created the stream or future, its writable end is
//! the host's too: it drops as the second of the pair, and the
//! producer that served it is dropped unpolled, because nobody is
//! left to read what it would produce. That is Wasmtime's
//! `host_drop_reader`, which its `close` reaches.
//!
//! An end the host piped to a consumer while a guest holds the
//! writable end closes too, while no write is in flight: the end
//! drops the same way, the idle writer is given the dropped result,
//! and the consumer is dropped unpolled, as Wasmtime's
//! `host_drop_reader` drops the reading side that holds it.
//!
//! A reader or a value that is neither closed, piped, nor lowered
//! into a guest leaks its end until the store drops, and a guest that
//! writes to it waits for good. Nothing closes it on the host's
//! behalf: the value names its end but cannot reach the store that
//! holds it.

use crate::concurrency::{EndId, EndKind};
use crate::error::Result;
use crate::store::{StoreContext, StoreContextInternalExt};

/// Close `reader`, a readable end of kind `kind`, in the store
/// `store` reaches, for a host value that names it and leaves the
/// value naming no end, as Wasmtime's close leaves `u32::MAX` in its
/// value.
///
/// A close of an end dropped already, whose record stays because a
/// guest holds the writable end, succeeds and does nothing, as
/// Wasmtime's does. A close of an end another value piped to a
/// consumer, for a guest's writable end with no write in flight,
/// drops the end and the consumer.
///
/// Fails with the not-present cause when the store holds no readable
/// end under `reader`: the value was closed already, or the end is
/// gone. Fails with the not-held cause when the end lives on in a
/// guest's table, or with a consumer that serves a write in flight or
/// that the host's own pipe holds.
/// [`TaskTables::held_by_host`](crate::concurrency::TaskTables::held_by_host)
/// states the rule and where it departs from Wasmtime. The value
/// names no end afterwards whether the close succeeds or fails, as
/// Wasmtime replaces the id before it looks the end up.
pub fn close_readable_end<T: 'static>(
    store: &mut StoreContext<'_, T>,
    reader: &mut EndId,
    kind: EndKind,
) -> Result<()> {
    let end = core::mem::replace(reader, EndId::CLOSED);
    let released = store
        .internal()
        .lock_tables()?
        .tasks
        .close_host_reader(end, kind)?;
    if let Some(host_end) = released {
        // The producer or consumer is dropped with no lock held,
        // because its own drop runs host code. Letting it go forgets
        // the waker kept for it too.
        let scheduler = store.internal().scheduler_mut();
        if host_end == end {
            drop(scheduler.release_host_reader(host_end));
        } else {
            drop(scheduler.release_host_writer(host_end));
        }
    }
    Ok(())
}
