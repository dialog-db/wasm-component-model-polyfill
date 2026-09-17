//! The bound the body of a host task carries.

use core::task::{Context, Poll};

use crate::error::Result;
use crate::value::Val;

use super::accessor::Accessor;

/// The bound the body of a host task carries.
///
/// A host task is one call of a host `async` function. Its body is
/// `'static` and does not borrow the store: the trampoline that
/// starts it returns to the guest, and the turns that follow poll
/// the body while they hold the store. A body that has to reach the
/// store's host data gets there through the accessor the store
/// hands into every poll, and only for the length of the closure
/// that accessor runs, so a value taken from the host data must be
/// cloned out of the closure. A body that needs nothing from the
/// store is a plain future, which
/// [`HostTask::from_future`](super::HostTask::from_future) wraps.
///
/// The `Send` half of the bound is the one per-target line. It is
/// required natively, so that a store stays `Send` as it is today.
/// It is absent in the browser: a body that awaits a JavaScript
/// promise is not `Send`, and awaiting one is the whole purpose of a
/// browser host function.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostTaskBody<T: 'static>: Send + 'static {
    /// Poll the body once, with `accessor` reaching the store this
    /// task belongs to and `context` carrying the waker of the turn
    /// that is polling. A body that completes answers with what the
    /// host call produced, which the store lowers into the subtask
    /// that awaits it.
    fn poll(
        &mut self,
        accessor: &Accessor<'_, T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>>;
}

/// The bound the body of a host task carries. See the native
/// definition for what it is and why the `Send` half is absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostTaskBody<T: 'static>: 'static {
    /// Poll the body once. See the native definition.
    fn poll(
        &mut self,
        accessor: &Accessor<'_, T>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Vec<Val>>>;
}
