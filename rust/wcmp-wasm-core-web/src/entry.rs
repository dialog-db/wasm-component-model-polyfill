//! A call from a store into a guest, while it runs.

use std::rc::Rc;

use crate::calls::Calls;
use crate::store::WebStore;

/// A call from a store into a guest, while it runs: the host functions of
/// the store reach the store until the entry drops.
///
/// [`Calls::enter`] makes one. Its drop puts back the store a guest ran in
/// before, and closes every frame of a host function that the call opened
/// and did not close.
pub struct Entry {
    calls: Rc<Calls>,
    previous: *mut WebStore,
    frames: usize,
}

impl Entry {
    /// The entry into `calls`, which puts back the store `previous` and
    /// the first `frames` frames when it drops.
    pub fn new(calls: Rc<Calls>, previous: *mut WebStore, frames: usize) -> Self {
        Self {
            calls,
            previous,
            frames,
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.calls.leave_entry(self.previous, self.frames);
    }
}
