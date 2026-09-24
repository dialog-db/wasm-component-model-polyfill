//! A guest's write to a stream or a future whose readable end the
//! host serves through a consumer, and the pipe that makes a consumer
//! the reading side of a readable end the host holds.
//!
//! [`pipe_readable_end`] routes a pipe by what the other end is:
//!
//! - A writable end a guest holds: the consumer joins the scheduler,
//!   keyed by the readable end, and the stream or future records the
//!   host as its reader. A write the guest already started is served
//!   by a host task the next turn polls.
//! - A writable end the host serves through a producer, because the
//!   host created the stream or future: the two are joined by a host
//!   task that copies from one to the other inside turns, with no
//!   guest involved (see `host_pipe`).
//! - A writable end that was dropped: the readable end drops at once,
//!   and the consumer with it, unpolled, as Wasmtime drops it.
//!
//! The host end runs as a host task. When a guest starts a write
//! against it, the copy built-in polls the end once, with the waker
//! of the turn that is running, before it returns:
//!
//! - A poll that comes out ready completes the write at once, so the
//!   built-in returns its result and never the blocked sentinel.
//! - A pending poll joins the store's host tasks, and the guest sees
//!   the blocked sentinel or blocks. A later turn polls the end again
//!   with the waker of the driver, and once the poll comes out ready
//!   the turn queues the completion, which fills the end's event.
//! - A poll that fails fails the built-in with the consumer's error
//!   when it is the first. A later one is the trap of the guest task
//!   that started the write, as the failure of a host call is the
//!   trap of the task that made the call.
//!
//! The consumer takes items out of the guest's memory during its
//! poll, through the source it is handed, and each poll records what
//! it took as the progress of the write whether it comes out ready or
//! pending: a consumer that took items and answered pending holds the
//! write back, which is the backpressure of Wasmtime's contract, and
//! its next poll is offered only what is left. An end whose consumer
//! is finished is let go of once the write that finished it
//! completes: the readable end drops, which tells the writer, and a
//! stream's completed write then reports the dropped result with the
//! progress it made.
//!
//! A guest that cancels a write the consumer has not answered wakes
//! the waker of the consumer's last pending poll, as Wasmtime wakes
//! its cancel waker. The next poll is asked to finish, and the write
//! then completes with what the consumer took: the copy's event
//! reports the cancelled result with that progress on a stream, and
//! on a future whose value the consumer did not take.

use core::task::{Context, Poll};

use crate::concurrency::{
    Accessor, CopyBuffer, CopyState, EndDirection, EndId, EndKind, HostConsumer, HostTask,
    HostTaskBody, TaskId, TaskTables,
};
use crate::error::{CopyCause, Error, Result};
use crate::internal::ErrorInternal;
use crate::store::{StoreContext, StoreContextInternalExt};
use crate::value::Val;

use super::host_pipe::start_host_pipe;

/// Make `consumer` the reading side of `reader`, a readable end of
/// kind `kind` the host holds, and give the end up. The routes are the
/// ones the module states.
///
/// Fails with the not-held cause when the store does not hold
/// `reader` for the host.
pub fn pipe_readable_end<T: 'static, H: HostConsumer<T>>(
    store: &mut StoreContext<'_, T>,
    reader: EndId,
    kind: EndKind,
    consumer: H,
) -> Result<()> {
    let not_held = || Error::Copy(CopyCause::NotHeldByHost { kind });
    let route = {
        let mut guard = store.internal().lock_tables()?;
        let tasks = &mut guard.tasks;
        let held = tasks.end(reader).is_some_and(|record| {
            record.direction == EndDirection::Readable && record.handle.is_none()
        });
        if !held {
            return Err(not_held());
        }
        let dropped = tasks
            .shared_record(reader)
            .is_some_and(|record| record.dropped);
        match tasks.host_counterpart(reader) {
            Some(writer) => Route::Pipe(writer),
            None => {
                tasks.serve_reader(reader, kind)?;
                if dropped {
                    tasks.release_host_end(reader)?;
                    Route::Dropped
                } else {
                    Route::Guest(writer_is_waiting(tasks, reader)?)
                }
            }
        }
    };
    match route {
        // The writer is gone, so nothing will reach the consumer: it
        // is dropped here, with no lock held, because its drop runs
        // host code.
        Route::Dropped => drop(consumer),
        Route::Pipe(writer) => {
            // The pipe takes the producer over, so a second pipe of
            // the same end finds none.
            let producer = store
                .internal()
                .scheduler_mut()
                .release_host_writer(writer)
                .ok_or_else(not_held)?;
            start_host_pipe(store, writer, producer, reader, consumer);
        }
        Route::Guest(waiting) => {
            store
                .internal()
                .scheduler_mut()
                .insert_host_reader(reader, Box::new(consumer));
            if waiting {
                // No task started the write through this pipe, so a
                // failure of the task that serves it reaches whichever
                // driver is running.
                serve_host_write(store, reader, None, false)?;
            }
        }
    }
    Ok(())
}

/// Where a pipe sends its consumer.
enum Route {
    /// The writable end dropped before the pipe.
    Dropped,
    /// The writable end is the host's own, served through a producer.
    Pipe(EndId),
    /// A guest holds the writable end; whether a write of it waits.
    Guest(bool),
}

/// Whether the writable end of `reader`'s stream or future has a
/// write in progress that nothing has answered yet.
fn writer_is_waiting(tasks: &TaskTables, reader: EndId) -> Result<bool> {
    let writer = tasks
        .shared_record(reader)
        .ok_or_else(|| Error::internal("a piped readable end has no shared record"))?
        .writable;
    Ok(tasks
        .end(writer)
        .is_some_and(|record| record.state.busy() && record.waitable.pending_event.is_none()))
}

/// Serve the write a guest started against `reader`, the readable end
/// the host serves, on behalf of `caller_task`, the guest task that
/// started it. With `now`, the end is polled once, here, with the
/// waker of the turn that is running: a poll that is ready completes
/// the write before this returns, and a pending one joins the store's
/// host tasks. A failure of either is the built-in's. Without `now`
/// the task joins the store's host tasks unpolled, and the next turn
/// polls it.
pub fn serve_host_write<T: 'static>(
    store: &mut StoreContext<'_, T>,
    reader: EndId,
    caller_task: Option<TaskId>,
    now: bool,
) -> Result<()> {
    let mut task = HostTask::copy(
        caller_task,
        move |store: &mut StoreContext<'_, T>, outcome: Result<Vec<Val>>| {
            // A poll that failed before it reached the store could not
            // forget the waker a pending poll kept; this forgets it.
            drop(store.internal().scheduler_mut().take_host_end_waker(reader));
            outcome?;
            complete_write(store, reader)
        },
        ReaderBody { reader },
    );
    if !now {
        store.internal().push_host_task(task);
        return Ok(());
    }
    let waker = store.internal().active_waker();
    match task.poll(store, &waker) {
        Poll::Ready(outcome) => task.lower(store, outcome),
        Poll::Pending => {
            store.internal().push_host_task(task);
            Ok(())
        }
    }
}

/// The body of the host task of a write against a host end: one poll
/// of the end's consumer per poll of the task.
struct ReaderBody {
    reader: EndId,
}

impl<T: 'static> HostTaskBody<T> for ReaderBody {
    fn poll(
        &mut self,
        accessor: &Accessor<T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>> {
        let reader = self.reader;
        match accessor.with(|store| poll_reader(store, reader, context)) {
            Ok(poll) => poll.map(|outcome| outcome.map(|()| Vec::new())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

/// Poll `reader` for the write in progress on its writer, lending the
/// consumer the store, and record what the poll took as the write's
/// progress. The end leaves the scheduler for the length of the poll,
/// because the consumer is given the whole store.
///
/// A writer with no write in progress has nothing to serve, and an
/// end the scheduler no longer holds has nothing to poll: both are
/// ready, and the completion that follows finds nothing to complete.
/// A poll of a write the guest is cancelling is asked to finish.
fn poll_reader<T: 'static>(
    store: &mut StoreContext<'_, T>,
    reader: EndId,
    context: &mut Context<'_>,
) -> Poll<Result<()>> {
    let (write, finish) = match writer_write(store, reader) {
        Ok(Some(write)) => write,
        Ok(None) => return Poll::Ready(Ok(())),
        Err(error) => return Poll::Ready(Err(error)),
    };
    let Some(mut end) = store.internal().scheduler_mut().take_host_reader(reader) else {
        return Poll::Ready(Ok(()));
    };
    let mut taken = 0;
    let poll = end.poll(context, store, &write, &mut taken, finish);
    let scheduler = store.internal().scheduler_mut();
    scheduler.insert_host_reader(reader, end);
    // A pending poll leaves its waker for a cancel of the write to
    // wake; a ready one leaves nothing to wake.
    match poll {
        Poll::Pending => scheduler.set_host_end_waker(reader, context.waker().clone()),
        Poll::Ready(_) => drop(scheduler.take_host_end_waker(reader)),
    }
    if taken > 0 {
        let recorded = store
            .internal()
            .lock_tables()
            .and_then(|mut guard| guard.tasks.record_host_take(reader, taken));
        if let Err(error) = recorded {
            return Poll::Ready(Err(error));
        }
    }
    poll
}

/// The write in progress on `reader`'s writer: a copy of its buffer,
/// and whether the guest is cancelling it, which asks the consumer's
/// poll to finish. `None` when the writer has no write in progress.
fn writer_write<T: 'static>(
    store: &mut StoreContext<'_, T>,
    reader: EndId,
) -> Result<Option<(CopyBuffer, bool)>> {
    let guard = store.internal().lock_tables()?;
    let writer = guard
        .tasks
        .shared_record(reader)
        .ok_or_else(|| Error::internal("a host end has no shared record"))?
        .writable;
    Ok(guard.tasks.end(writer).and_then(|record| {
        let cancelling = record.state == CopyState::Cancelling;
        record
            .buffer
            .as_ref()
            .map(|buffer| (buffer.clone(), cancelling))
    }))
}

/// Complete the write in progress on `reader`'s writer, now that a
/// poll of the consumer came out ready. An end whose consumer is
/// finished is let go of afterwards.
fn complete_write<T: 'static>(store: &mut StoreContext<'_, T>, reader: EndId) -> Result<()> {
    let Some(end) = store.internal().scheduler_mut().take_host_reader(reader) else {
        return Ok(());
    };
    let over = end.finished();
    let completed = store.internal().lock_tables().and_then(|mut guard| {
        let writing = guard
            .tasks
            .shared_record(reader)
            .and_then(|record| guard.tasks.end(record.writable))
            .is_some_and(|record| record.buffer.is_some());
        if writing {
            guard.tasks.finish_host_copy(reader, 0)?;
        }
        if over {
            guard.tasks.release_host_end(reader)?;
        }
        Ok(())
    });
    let scheduler = store.internal().scheduler_mut();
    if over {
        // The end is let go of for good, and no waker is left kept
        // for it. The consumer is dropped with no lock held.
        drop(scheduler.release_host_reader(reader));
        drop(end);
    } else {
        scheduler.insert_host_reader(reader, end);
    }
    completed
}
