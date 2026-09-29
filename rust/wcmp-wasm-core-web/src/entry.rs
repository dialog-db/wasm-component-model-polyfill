//! A synchronous call from a store into a guest, while it runs.

use std::rc::Rc;

use crate::calls::Calls;

/// A synchronous call from a store into a guest, while it runs: the host
/// functions of the store reach the store through its lease until the
/// entry leaves.
///
/// [`Calls::enter`] makes one. When it leaves, or drops, the lease ends,
/// and every frame of a host function that the call opened and did not
/// close closes.
pub struct Entry {
    calls: Rc<Calls>,
    depth: Option<usize>,
}

impl Entry {
    /// The entry of the lease at `depth` of `calls`.
    pub fn new(calls: Rc<Calls>, depth: usize) -> Self {
        Self {
            calls,
            depth: Some(depth),
        }
    }

    /// Ends the lease, and answers the error of the host function that
    /// trapped the call, where one did.
    ///
    /// A host function that fails traps the guest at once, and the trap
    /// unwinds WebAssembly frames alone up to the call. So the error found
    /// here is the error of the call's own trap.
    pub fn leave(mut self) -> Option<anyhow::Error> {
        self.depth
            .take()
            .and_then(|depth| self.calls.leave_entry(depth))
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(depth) = self.depth.take() {
            self.calls.leave_entry(depth);
        }
    }
}
