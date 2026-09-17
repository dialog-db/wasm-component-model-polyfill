//! The target's filling of the scheduler's suspend capability.

use crate::error::Result;
use crate::store::Store;

/// The target's filling of the scheduler's suspend capability.
///
/// The reference lets a running guest thread block inside a
/// built-in, and Wasmtime serves that by suspending the fiber the
/// guest runs on. The polyfill runs the guest on the one real stack,
/// so a host trampoline that must block has no way back to the
/// scheduler without unwinding the guest. A provider is what gives
/// it one: it switches the stack the guest runs on, parks the
/// current thread until the readiness condition holds, and returns
/// when the thread resumes.
///
/// The polyfill fills the capability on neither target today. In the
/// browser JavaScript Promise Integration is the intended provider:
/// the host's entry into the guest becomes a promising call and a
/// blocking built-in becomes a suspending import, whose promise
/// suspends the guest until it resolves. The native target has no
/// intended provider. Neither is designed here.
///
/// Two consequences of that shape hold for every provider, and the
/// seam is built around them:
///
/// - A suspension traps if a frame that is not WebAssembly sits
///   between the promising entry and the suspending import, and in
///   the browser every host trampoline puts a JavaScript frame on
///   the stack. Guest code is therefore entered only from the
///   scheduler while a provider is present, never from inside a
///   trampoline — which is why the seam consults the slot before it
///   falls back to a nested turn, and why the two never run
///   together. The one exception is a frame that cannot block: a
///   synchronous resource destructor or a `post-return` run from a
///   trampoline.
/// - A suspended thread resumes outside any poll of a driver. The
///   scheduler's state is therefore reachable from a trampoline
///   without a driver on the stack, through the store's handle
///   tables.
pub trait SuspendProvider<T: 'static>: 'static {
    /// Suspend the current guest thread until `condition` holds.
    ///
    /// The provider returns `Ok(())` once the thread has resumed
    /// with the condition true. It returns a scheduler error when it
    /// cannot serve the block — a thread that must not block, or a
    /// condition nothing can still make true.
    ///
    /// `condition` is consulted against the store, so a provider
    /// that hands control back to the scheduler between checks sees
    /// whatever the turns in between produced.
    fn suspend(
        &mut self,
        store: &mut Store<T>,
        condition: &mut dyn FnMut(&mut Store<T>) -> bool,
    ) -> Result<()>;
}
