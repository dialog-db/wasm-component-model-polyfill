//! The bound the future of a host `async` function carries.

use core::future::Future;

use crate::error::Result;
use crate::value::Val;

/// The bound the future of a host `async` function carries.
///
/// One call of a host `async` function produces a future the store
/// owns and polls. The runtime layer hands a host trampoline a
/// synchronous closure and nothing else, so the trampoline cannot
/// run the future itself: it gives the future to the store and
/// returns to the guest, and later turns poll it. The future is
/// therefore `'static` and does not borrow the store.
///
/// The `Send` half of the bound is the one per-target line. It is
/// required natively, so that a store stays `Send` as it is today.
/// It is absent in the browser: a JavaScript promise wrapped as a
/// future is not `Send`, and awaiting one is the whole purpose of a
/// browser host function.
#[cfg(not(target_arch = "wasm32"))]
pub trait HostFuture: Future<Output = Result<Vec<Val>>> + Send + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<F> HostFuture for F where F: Future<Output = Result<Vec<Val>>> + Send + 'static {}

/// The bound the future of a host `async` function carries. See the
/// native definition for what it is and why the `Send` half is
/// absent here.
#[cfg(target_arch = "wasm32")]
pub trait HostFuture: Future<Output = Result<Vec<Val>>> + 'static {}

#[cfg(target_arch = "wasm32")]
impl<F> HostFuture for F where F: Future<Output = Result<Vec<Val>>> + 'static {}
