// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A future reader that closes its future when it drops.

use super::accessor::Accessor;
use super::future_reader::FutureReader;

/// A [`FutureReader`] paired with an [`Accessor`], which closes the
/// future when it drops. The name is Wasmtime's.
///
/// [`FutureReader::guard`] makes one, and so does
/// [`GuardedFutureReader::new`]. [`into_future`](Self::into_future)
/// gives the reader back, and the guard then closes nothing.
///
/// The guard closes through the accessor, which reaches the store
/// only inside a poll of it: the `run_concurrent` closure, a host
/// task's body, or a host `async` function. A guard dropped there
/// closes the future, as [`FutureReader::close_with`] does. A guard
/// dropped anywhere else cannot reach the store, and neither can one
/// dropped inside another reach of the same store, so its end leaks
/// the way a reader dropped without a close leaks: until the store
/// drops, with a guest that writes to the future waiting for good. A
/// close that fails is not reported, because a drop has nowhere to
/// report it.
///
/// That leak departs from Wasmtime. Its guard closes through
/// `Accessor::with` too, which panics when no poll of the store is
/// running and when another reach of the store is, so a Wasmtime
/// guard dropped there panics, and one whose close fails trips a
/// debug assertion. The polyfill's accessor reports both cases as
/// errors instead, and the guard lets the end leak rather than turn
/// an error into a panic in a drop, which aborts the process when the
/// drop runs while a thread unwinds.
pub struct GuardedFutureReader<T, D: 'static> {
    /// The reader, until the guard drops or gives it back.
    reader: Option<FutureReader<T>>,
    accessor: Accessor<D>,
}

impl<T, D: 'static> GuardedFutureReader<T, D> {
    /// Pair `reader` with `accessor`, which must be an accessor of the
    /// store that holds the reader's future. The name is Wasmtime's.
    pub fn new(accessor: Accessor<D>, reader: FutureReader<T>) -> Self {
        Self {
            reader: Some(reader),
            accessor,
        }
    }

    /// Give the reader back, and close nothing. The name is
    /// Wasmtime's.
    pub fn into_future(self) -> FutureReader<T> {
        self.into()
    }
}

impl<T, D: 'static> From<GuardedFutureReader<T, D>> for FutureReader<T> {
    fn from(mut guard: GuardedFutureReader<T, D>) -> Self {
        guard
            .reader
            .take()
            .expect("a guard holds its reader until it drops or gives it back")
    }
}

impl<T, D: 'static> Drop for GuardedFutureReader<T, D> {
    fn drop(&mut self) {
        if let Some(mut reader) = self.reader.take() {
            // A guard dropped outside a poll of its store cannot reach
            // it, and its end leaks, as the type states.
            drop(reader.close_with(&self.accessor));
        }
    }
}
