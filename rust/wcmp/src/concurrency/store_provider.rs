//! The provider a store runs its guest threads through.

use core::task::{Poll, Waker};

use crate::error::Result;
use crate::runtime_layer::{Func as RuntimeFunc, FuncType, Val as RuntimeVal};
use crate::store::StoreContext;
use crate::suspend_provider_kind::SuspendProviderKind;

use super::entry_status::EntryStatus;
#[cfg(target_arch = "wasm32")]
use super::jspi_provider::JspiProvider;
use super::stack_switching_provider::StackSwitchingProvider;
use super::suspend_provider::SuspendProvider;
use super::thread_id::ThreadId;

/// The provider a store runs its guest threads through: the one the
/// engine selected, instantiated in the store when the store was
/// constructed.
///
/// Both providers meet the [`SuspendProvider`] contract, and the
/// scheduler reaches either through it. They differ in one thing the
/// scheduler must know: whether a resume runs the thread inside the
/// call, as the stack-switching provider does, or on a microtask
/// after it, as the JSPI provider does. A thread resumed on a
/// microtask runs with no host frame below it, so a resume may be made
/// only where the store runs no guest code, and a trampoline that has
/// to resume a thread from inside a guest call suspends its own
/// thread first. [`may_resume_here`](Self::may_resume_here) and
/// [`may_suspend_here`](Self::may_suspend_here) answer those two
/// questions.
///
/// A clone is a handle to the same provider, which a caller takes
/// out of the store before calling it with the store beside it.
#[derive(Clone)]
pub enum StoreProvider {
    /// The stack-switching provider.
    StackSwitching(StackSwitchingProvider),
    /// The JSPI provider, in the browser.
    #[cfg(target_arch = "wasm32")]
    Jspi(JspiProvider),
}

impl StoreProvider {
    /// Which provider this is.
    pub fn kind(&self) -> SuspendProviderKind {
        match self {
            Self::StackSwitching(_) => SuspendProviderKind::StackSwitching,
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(_) => SuspendProviderKind::Jspi,
        }
    }

    /// Whether a resume of this provider runs the thread on a
    /// microtask after the call, rather than inside it.
    pub fn resumes_later(&self) -> bool {
        match self {
            Self::StackSwitching(_) => false,
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(_) => true,
        }
    }

    /// Whether a thread may be resumed from the frame that runs now.
    /// The stack-switching provider resumes one anywhere. The JSPI
    /// provider resumes one only where the store runs no guest code,
    /// which is a turn of a driver.
    // The store is read by the JSPI provider alone, which exists in
    // the browser alone.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub fn may_resume_here<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        match self {
            Self::StackSwitching(_) => true,
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.at_rest(store),
        }
    }

    /// Whether a shim called from the host function that runs now may
    /// suspend the stack it runs on, which is what a trampoline that
    /// cannot resume a thread where it stands does instead. Only the
    /// JSPI provider needs to.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub fn may_suspend_here<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        match self {
            Self::StackSwitching(_) => false,
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.may_suspend(store),
        }
    }

    /// Keep `store` allocated, and mark it dropped, when it drops with
    /// a resumed thread yet to run, which the JSPI provider runs on a
    /// microtask that reaches the store.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub fn retain_if_resuming<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        match self {
            Self::StackSwitching(_) => {}
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.retain_if_resuming(store),
        }
    }

    /// Free `store`, which [`retain_if_resuming`](Self::retain_if_resuming)
    /// kept for a resumed thread, once that thread stopped reaching
    /// it. The thread calls this from its shim's try.
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub fn release_dropped<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        match self {
            Self::StackSwitching(_) => {}
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.release_dropped(store),
        }
    }

    /// Make the shims of the blocking built-ins in `hosts`, one for
    /// each, in the order given. Each entry names the shim's type, the
    /// built-in's try, and its finish.
    pub fn shims<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        hosts: &[(FuncType, RuntimeFunc, RuntimeFunc)],
    ) -> Result<Vec<RuntimeFunc>> {
        match self {
            Self::StackSwitching(provider) => provider.shims(store, hosts),
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.shims(store, hosts),
        }
    }
}

impl<T: 'static> SuspendProvider<T> for StoreProvider {
    fn start(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        match self {
            Self::StackSwitching(provider) => provider.start(store, thread, entry, ty, args),
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.start(store, thread, entry, ty, args),
        }
    }

    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus> {
        match self {
            Self::StackSwitching(provider) => provider.resume(store, thread),
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.resume(store, thread),
        }
    }

    fn poll_stop(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        waker: &Waker,
    ) -> Poll<Result<EntryStatus>> {
        match self {
            Self::StackSwitching(provider) => provider.poll_stop(store, thread, waker),
            #[cfg(target_arch = "wasm32")]
            Self::Jspi(provider) => provider.poll_stop(store, thread, waker),
        }
    }
}
