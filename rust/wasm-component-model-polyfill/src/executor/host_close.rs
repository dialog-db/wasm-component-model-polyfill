//! The host's close of a readable end it holds.
//!
//! A host that holds a stream or a future it will not read closes the
//! reader, which drops the readable end as a guest's
//! `stream.drop-readable` or `future.drop-readable` drops one. A write
//! in progress on the writable end completes with the dropped result,
//! and a later write sees it at once. When the host created the
//! stream or future, its writable end is the host's too: it drops as
//! the second of the pair, and the producer that served it is dropped
//! unpolled, because nobody is left to read what it would produce.
//! That is Wasmtime's `close`.
//!
//! A reader that is neither closed, piped, nor lowered into a guest
//! leaks its end until the store drops, and a guest that writes to it
//! waits for good. Nothing closes it on the host's behalf: the reader
//! names its end but cannot reach the store that holds it.

use crate::concurrency::{EndId, EndKind};
use crate::error::{CopyCause, Error, Result};
use crate::store::{StoreContext, StoreContextInternalExt};

/// Close `reader`, a readable end of kind `kind` the host holds, in
/// the store `store` reaches.
///
/// Fails with the not-held cause when the store does not hold
/// `reader` for the host: it was closed, piped, or lowered already,
/// or it names an end of another store. A reader whose writable end
/// the host serves through a producer that a pipe has taken over is
/// refused the same way, because the pipe, not the reader, holds the
/// stream now.
pub fn close_readable_end<T: 'static>(
    store: &mut StoreContext<'_, T>,
    reader: EndId,
    kind: EndKind,
) -> Result<()> {
    let not_held = || Error::Copy(CopyCause::NotHeldByHost { kind });
    let writer = {
        let guard = store.internal().lock_tables()?;
        if !guard.tasks.held_by_host(reader) {
            return Err(not_held());
        }
        guard.tasks.host_counterpart(reader)
    };
    if let Some(writer) = writer
        && !store.internal().scheduler().holds_host_writer(writer)
    {
        return Err(not_held());
    }
    let released = store
        .internal()
        .lock_tables()?
        .tasks
        .close_host_reader(reader, kind)?;
    if let Some(writer) = released {
        // The producer is dropped with no lock held, because its own
        // drop runs host code. Letting it go forgets the waker kept
        // for it too.
        drop(store.internal().scheduler_mut().release_host_writer(writer));
    }
    Ok(())
}
