//! The target's filling of the scheduler's suspend capability.

use crate::error::Result;
use crate::store::StoreContext;

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
///   without a driver on the stack. It rides in the core store's
///   data, which the runtime layer hands every trampoline as a
///   context, so a blocking built-in reaches the seam, the queues,
///   and the host tasks through [`StoreContext`] and captures
///   nothing.
///
/// One obligation falls on the first provider to fill the seam. A
/// synchronous lower of a host `async` function that blocks keeps
/// its host task in the trampoline's frame rather than among the
/// store's host tasks, because the call the task belongs to is
/// still on the guest's stack. While the thread is suspended the
/// store therefore holds no record of that pending body, and a
/// driver that asks the store whether it holds work a turn can
/// carry forward is told no. A provider that lets a driver of the
/// same store run while such a frame is suspended must keep the
/// store aware of the pending body — by parking a record of it
/// there, or by admitting no turn until the frame resumes. A
/// provider that does neither leaves that driver to go idle with
/// the task unresolved, which raises the deadlock cause.
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
        store: &mut StoreContext<'_, T>,
        condition: &mut dyn FnMut(&mut StoreContext<'_, T>) -> bool,
    ) -> Result<()>;
}
