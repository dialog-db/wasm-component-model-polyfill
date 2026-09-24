//! A stream reader that closes its stream when it drops.

use super::accessor::Accessor;
use super::stream_reader::StreamReader;

/// A [`StreamReader`] paired with an [`Accessor`], which closes the
/// stream when it drops. The name is Wasmtime's.
///
/// [`StreamReader::guard`] makes one, and so does
/// [`GuardedStreamReader::new`]. [`into_stream`](Self::into_stream)
/// gives the reader back, and the guard then closes nothing.
///
/// The guard closes through the accessor, which reaches the store
/// only inside a poll of it: the `run_concurrent` closure, a host
/// task's body, or a host `async` function. A guard dropped there
/// closes the stream, as [`StreamReader::close_with`] does. A guard
/// dropped anywhere else cannot reach the store, and neither can one
/// dropped inside another reach of the same store, so its end leaks
/// the way a reader dropped without a close leaks: until the store
/// drops, with a guest that writes to the stream waiting for good. A
/// close that fails is not reported, because a drop has nowhere to
/// report it.
pub struct GuardedStreamReader<T, D: 'static> {
    /// The reader, until the guard drops or gives it back.
    reader: Option<StreamReader<T>>,
    accessor: Accessor<D>,
}

impl<T, D: 'static> GuardedStreamReader<T, D> {
    /// Pair `reader` with `accessor`, which must be an accessor of the
    /// store that holds the reader's stream. The name is Wasmtime's.
    pub fn new(accessor: Accessor<D>, reader: StreamReader<T>) -> Self {
        Self {
            reader: Some(reader),
            accessor,
        }
    }

    /// Give the reader back, and close nothing. The name is
    /// Wasmtime's.
    pub fn into_stream(self) -> StreamReader<T> {
        self.into()
    }
}

impl<T, D: 'static> From<GuardedStreamReader<T, D>> for StreamReader<T> {
    fn from(mut guard: GuardedStreamReader<T, D>) -> Self {
        guard
            .reader
            .take()
            .expect("a guard holds its reader until it drops or gives it back")
    }
}

impl<T, D: 'static> Drop for GuardedStreamReader<T, D> {
    fn drop(&mut self) {
        if let Some(mut reader) = self.reader.take() {
            // A guard dropped outside a poll of its store cannot reach
            // it, and its end leaks, as the type states.
            drop(reader.close_with(&self.accessor));
        }
    }
}
