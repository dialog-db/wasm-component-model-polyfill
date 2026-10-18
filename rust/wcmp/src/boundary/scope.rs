// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "boundary-counters")]
use super::clock::now_ms;
#[cfg(feature = "boundary-counters")]
use super::open_scope::OpenScope;
#[cfg(feature = "boundary-counters")]
use super::state::STATE;
use super::{BoundarySink, ImportName};

/// A stretch of a store's work whose counts go to that store, and to
/// one host import of it when the scope names one, while the guard
/// lives.
///
/// Scopes nest, and each count goes to the innermost scope open: a host
/// import that calls back into a guest that calls another import counts
/// the inner call to the inner import alone. When a scope opens or
/// closes, the clocks that run are settled into the scope that was
/// innermost until then, so each scope counts only the time that passed
/// while it was the innermost. A scope adds what it counted to its
/// store as it closes.
pub struct BoundaryScope {
    /// The scope's identity in the thread's state.
    #[cfg(feature = "boundary-counters")]
    id: u64,
}

impl BoundaryScope {
    /// Open a scope that counts for the store `sink` belongs to, and for
    /// `import` when it names one.
    #[inline]
    pub fn enter(sink: &BoundarySink, import: &ImportName) -> Self {
        #[cfg(feature = "boundary-counters")]
        {
            let id = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if state.timing() {
                    state.settle(now_ms());
                }
                let id = state.next_scope;
                state.next_scope += 1;
                state.scopes.push(OpenScope {
                    id,
                    sink: sink.clone(),
                    import: import.get().cloned(),
                    counts: Default::default(),
                });
                id
            });
            BoundaryScope { id }
        }
        #[cfg(not(feature = "boundary-counters"))]
        {
            let _ = (sink, import);
            BoundaryScope {}
        }
    }
}

#[cfg(feature = "boundary-counters")]
impl Drop for BoundaryScope {
    fn drop(&mut self) {
        let closed = STATE.with(|state| {
            let mut state = state.borrow_mut();
            // A scope closes by its identity rather than off the top.
            // Guards open and close in order, because a stack the suspend
            // provider sets aside holds only WebAssembly frames and so no
            // guard, so this only keeps a guard that dropped out of order
            // from closing another scope.
            let position = state.scopes.iter().rposition(|scope| scope.id == self.id)?;
            if position + 1 == state.scopes.len() && state.timing() {
                state.settle(now_ms());
            }
            Some(state.scopes.remove(position))
        });
        if let Some(scope) = closed {
            scope.sink.add(scope.import.as_ref(), &scope.counts);
        }
    }
}
