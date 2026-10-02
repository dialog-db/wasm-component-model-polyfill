// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A guest's read of a stream or a future whose writable end the host
//! serves through a producer.
//!
//! The host end runs as a host task. When a guest starts a read
//! against it, the copy built-in polls the end once, with the waker
//! of the turn that is running, before it returns:
//!
//! - A poll that comes out ready is delivered at once: what the
//!   producer produced moves into the guest's buffer and the read
//!   completes, so the built-in returns its result and never the
//!   blocked sentinel.
//! - A pending poll joins the store's host tasks, and the guest sees
//!   the blocked sentinel or blocks. A later turn polls the end again
//!   with the waker of the driver, and once the poll comes out ready
//!   the turn queues the delivery, which fills the end's event.
//! - A poll that fails fails the built-in with the producer's error
//!   when it is the first. A later one is a trap, as the failure
//!   of a host call is, and so is a delivery that fails: it poisons
//!   the store and ends the driver whose turn meets it.
//!
//! A poll is made only when nothing waits with the end: items the
//! producer delivered beyond what the reader could take satisfy the
//! reader's later reads first. The delivery moves as many as the
//! read can take through a boundary context over the reader's
//! memory, and the producer lowers each one as its `ComponentValue`
//! lowers it. An end whose producer is finished and which has no
//! item left is let go of: its writable end drops, which tells the
//! reader the stream ended, and a future's end drops once its value
//! is delivered.
//!
//! A guest that cancels a read the producer has not answered wakes
//! the waker of the producer's last pending poll, as Wasmtime wakes
//! its cancel waker. The next poll is asked to finish, and its
//! delivery completes the read with what the producer delivered:
//! the copy's event then reports the cancelled result with that
//! progress on a stream, and on a future that received no value.

use core::task::{Context, Poll};

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::size_of;
use crate::concurrency::{Accessor, CopyState, EndId, HostTask, HostTaskBody};
use crate::error::{AbiPosition, Error, Result};
use crate::internal::ErrorInternal;
use crate::runtime_layer::AsContextMut;
use crate::store::{StoreContext, StoreContextInternalExt};
use crate::value::Val;

/// The argument slot a delivery's failure is labelled with: the
/// pointer of the read the items are lowered into.
const POINTER_ARGUMENT: AbiPosition = AbiPosition::Argument(1);

/// Serve the read a guest just started against `writer`, the writable
/// end the host serves. The end is polled once, here and now, with the
/// waker of the turn that is running: a poll that is ready is
/// delivered before this returns, and a pending one joins the store's
/// host tasks. A failure of either is the built-in's.
pub fn serve_host_read<T: 'static>(store: &mut StoreContext<'_, T>, writer: EndId) -> Result<()> {
    let mut task = HostTask::copy(
        move |store: &mut StoreContext<'_, T>, outcome: Result<Vec<Val>>| {
            // A poll that failed before it reached the store could not
            // forget the waker a pending poll kept; this forgets it.
            drop(store.internal().scheduler_mut().take_host_end_waker(writer));
            outcome?;
            deliver(store, writer)
        },
        WriterBody { writer },
    );
    let waker = store.internal().active_waker();
    match task.poll(store, &waker) {
        Poll::Ready(outcome) => task.lower(store, outcome),
        Poll::Pending => {
            store.internal().push_host_task(task);
            Ok(())
        }
    }
}

/// The body of the host task of a read against a host end: one poll
/// of the end's producer per poll of the task.
struct WriterBody {
    writer: EndId,
}

impl<T: 'static> HostTaskBody<T> for WriterBody {
    fn poll(
        &mut self,
        accessor: &Accessor<T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>> {
        let writer = self.writer;
        match accessor.with(|store| poll_writer(store, writer, context)) {
            Ok(poll) => poll.map(|outcome| outcome.map(|()| Vec::new())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

/// Poll `writer` for the read in progress on its reader, lending the
/// producer the store. The end leaves the scheduler for the length of
/// the poll, because the producer is given the whole store.
///
/// A reader with no copy in progress has nothing to serve, and an end
/// the scheduler no longer holds has nothing to poll: both are ready,
/// and the delivery that follows finds nothing to move. A poll of a
/// read the guest is cancelling is asked to finish.
fn poll_writer<T: 'static>(
    store: &mut StoreContext<'_, T>,
    writer: EndId,
    context: &mut Context<'_>,
) -> Poll<Result<()>> {
    let (remaining, finish) = match reader_read(store, writer) {
        Ok(Some(read)) => read,
        Ok(None) => return Poll::Ready(Ok(())),
        Err(error) => return Poll::Ready(Err(error)),
    };
    let Some(mut end) = store.internal().scheduler_mut().take_host_writer(writer) else {
        return Poll::Ready(Ok(()));
    };
    let poll = end.poll(context, store, Some(remaining as usize), finish);
    let scheduler = store.internal().scheduler_mut();
    scheduler.insert_host_writer(writer, end);
    // A pending poll leaves its waker for a cancel of the read to
    // wake; a ready one leaves nothing to wake.
    match poll {
        Poll::Pending => scheduler.set_host_end_waker(writer, context.waker().clone()),
        Poll::Ready(_) => drop(scheduler.take_host_end_waker(writer)),
    }
    poll
}

/// The read in progress on `writer`'s reader: the count it can still
/// take, and whether the guest is cancelling it, which asks the
/// producer's poll to finish. `None` when the reader has no read in
/// progress.
fn reader_read<T: 'static>(
    store: &mut StoreContext<'_, T>,
    writer: EndId,
) -> Result<Option<(u32, bool)>> {
    let guard = store.internal().lock_tables()?;
    let reader = guard
        .tasks
        .shared_record(writer)
        .ok_or_else(|| Error::internal("a host end has no shared record"))?
        .readable;
    Ok(guard.tasks.end(reader).and_then(|record| {
        let cancelling = record.state == CopyState::Cancelling;
        record
            .buffer
            .as_ref()
            .map(|buffer| (buffer.remain(), cancelling))
    }))
}

/// Move what waits with `writer` into the buffer of the read in
/// progress on its reader, as much as the read can take, and complete
/// the read. An end that is over is let go of afterwards.
fn deliver<T: 'static>(store: &mut StoreContext<'_, T>, writer: EndId) -> Result<()> {
    let read = {
        let guard = store.internal().lock_tables()?;
        let reader = guard
            .tasks
            .shared_record(writer)
            .ok_or_else(|| Error::internal("a host end has no shared record"))?
            .readable;
        guard
            .tasks
            .end(reader)
            .and_then(|record| record.buffer.as_ref())
            .map(|buffer| {
                (
                    buffer.options.clone(),
                    buffer.abi_state.clone(),
                    buffer.payload.clone(),
                    buffer.pointer,
                    buffer.progress,
                    buffer.remain(),
                )
            })
    };
    let Some((options, abi_state, payload, pointer, progress, remain)) = read else {
        return Ok(());
    };
    let Some(mut end) = store.internal().scheduler_mut().take_host_writer(writer) else {
        return Ok(());
    };
    let count = (remain as usize).min(end.waiting());
    let moved = match &payload {
        Some(ty) if count > 0 => {
            let tables = store.internal().tables_handle();
            BoundaryInstance::resolve(&options, &abi_state, &tables).and_then(
                |(options, instance)| {
                    let mut cx = BoundaryContext::new(
                        store.internal().runtime_mut().as_context_mut(),
                        options,
                        instance,
                        None,
                    );
                    let size = size_of(ty);
                    let offset = pointer as usize + progress as usize * size;
                    cx.lowering_within(offset, count * size, POINTER_ARGUMENT, ty, |cx| {
                        end.deliver(cx, offset, ty, count)
                    })
                },
            )
        }
        _ => Ok(()),
    };
    let over = end.finished() && end.waiting() == 0;
    if let Err(error) = moved {
        store
            .internal()
            .scheduler_mut()
            .insert_host_writer(writer, end);
        return Err(error);
    }
    {
        let mut guard = store.internal().lock_tables()?;
        guard.tasks.finish_host_copy(writer, count as u32)?;
        if over {
            guard.tasks.release_host_end(writer)?;
        }
    }
    let scheduler = store.internal().scheduler_mut();
    if over {
        // The end is let go of for good, and no waker is left kept
        // for it. The producer is dropped with no lock held.
        drop(scheduler.release_host_writer(writer));
        drop(end);
    } else {
        scheduler.insert_host_writer(writer, end);
    }
    Ok(())
}
