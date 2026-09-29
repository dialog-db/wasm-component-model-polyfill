//! A stream or a future the host created and piped to a consumer of
//! its own, which copies from the producer to the consumer inside
//! turns with no guest involved.
//!
//! The pipe runs as one host task. Each poll of it drives the two
//! sides in turn until one of them is pending: the consumer is polled
//! while items wait for it or while its last poll answered pending,
//! and the producer is polled for more once the consumer has taken
//! every item and answered ready, with a destination whose remaining
//! count is `None`, which says that the reader is the host. The items
//! cross from the producer to the consumer as the values they cross
//! a boundary as, so the two sides need only name the same component
//! type. Wasmtime's host-to-host path moves them without converting,
//! and requires the same Rust type.
//!
//! The pipe ends when the producer is done and the consumer has taken
//! everything it delivered, when the consumer answers that it is
//! over, or when either side fails. Both ends of the stream or future
//! are then let go of, and the producer and the consumer are dropped.
//! A failure belongs to no guest task, so it reaches whichever driver
//! is running, as `Store::run_concurrent` states.

use core::task::{Context, Poll};

use crate::concurrency::{Accessor, EndId, HostConsumer, HostTask, HostTaskBody, HostWriter};
use crate::error::Result;
use crate::internal::SourceInternal;
use crate::linker::ComponentValue;
use crate::store::{StoreContext, StoreContextInternalExt};
use crate::value::Val;

/// Start the pipe from `producer`, which serves `writer`, to
/// `consumer`, the reading side of `reader`: a host task the next
/// turn polls.
pub fn start_host_pipe<T: 'static, H: HostConsumer<T>>(
    store: &mut StoreContext<'_, T>,
    writer: EndId,
    producer: Box<dyn HostWriter<T>>,
    reader: EndId,
    consumer: H,
) {
    let body = PipeBody {
        writer,
        reader,
        producer: Some(producer),
        consumer: Some(consumer),
        offered: Vec::new(),
        consuming: false,
        earlier: 0,
        produced_all: false,
    };
    let task = HostTask::copy(
        |_store: &mut StoreContext<'_, T>, outcome: Result<Vec<Val>>| outcome.map(drop),
        body,
    )
    .host_only();
    store.internal().push_host_task(task);
}

/// The body of a pipe's host task.
struct PipeBody<T: 'static, H: HostConsumer<T>> {
    writer: EndId,
    reader: EndId,
    /// The producer, until the pipe ends.
    producer: Option<Box<dyn HostWriter<T>>>,
    /// The consumer, until the pipe ends.
    consumer: Option<H>,
    /// The items the producer delivered that the consumer has not
    /// taken yet, in order.
    offered: Vec<H::Item>,
    /// Whether the consumer's last poll answered pending, so that it
    /// is polled again before the producer is.
    consuming: bool,
    /// How many items the consumer took in polls that answered
    /// pending since it last answered ready.
    earlier: usize,
    /// Whether the producer will deliver nothing more.
    produced_all: bool,
}

impl<T: 'static, H: HostConsumer<T>> HostTaskBody<T> for PipeBody<T, H> {
    fn poll(
        &mut self,
        accessor: &Accessor<T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>> {
        let polled = accessor.with(|store| {
            let poll = self.step(store, context);
            if poll.is_ready() {
                // Whatever the pipe answered, it is over: let both
                // ends go, and keep the first failure.
                let closed = self.close(store);
                return poll.map(|outcome| outcome.and(closed));
            }
            poll
        });
        match polled {
            Ok(poll) => poll.map(|outcome| outcome.map(|()| Vec::new())),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

impl<T: 'static, H: HostConsumer<T>> PipeBody<T, H> {
    /// Drive the two sides until one of them is pending or the pipe is
    /// over.
    fn step(&mut self, store: &mut StoreContext<'_, T>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let (Some(producer), Some(consumer)) = (&mut self.producer, &mut self.consumer) else {
            return Poll::Ready(Ok(()));
        };
        loop {
            if consumer.finished() {
                return Poll::Ready(Ok(()));
            }
            if self.consuming || !self.offered.is_empty() {
                let before = self.offered.len();
                let source = crate::concurrency::Source::host(&mut self.offered);
                let poll = consumer.consume(cx, store, source, false, self.earlier);
                self.earlier += before - self.offered.len();
                match poll {
                    Poll::Pending => {
                        self.consuming = true;
                        return Poll::Pending;
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(())) => {
                        self.consuming = false;
                        self.earlier = 0;
                        continue;
                    }
                }
            }
            if self.produced_all {
                return Poll::Ready(Ok(()));
            }
            match producer.poll(cx, store, None, false) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
            for item in producer.take_items() {
                match H::Item::from_val(&item) {
                    Ok(item) => self.offered.push(item),
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }
            self.produced_all = producer.finished();
        }
    }

    /// Let go of both ends of the stream or future, and drop the
    /// producer and the consumer once no lock is held.
    fn close(&mut self, store: &mut StoreContext<'_, T>) -> Result<()> {
        let released = store.internal().lock_tables().and_then(|mut guard| {
            for end in [self.writer, self.reader] {
                if guard.tasks.end(end).is_some() {
                    guard.tasks.release_host_end(end)?;
                }
            }
            Ok(())
        });
        drop(self.producer.take());
        drop(self.consumer.take());
        drop(core::mem::take(&mut self.offered));
        released
    }
}
