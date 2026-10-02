// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The provider a store runs its guest threads through.

use core::task::{Poll, Waker};

use crate::error::Result;
use crate::runtime_layer::{Func as RuntimeFunc, FuncType, Val as RuntimeVal};
use crate::store::StoreContext;
use crate::suspend_provider_kind::SuspendProviderKind;

use super::entry_status::EntryStatus;
use super::host_suspension_provider::HostSuspensionProvider;
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
/// call, as the stack-switching provider does, or once the driver of
/// the store awaits it, as the host-suspension provider does. A thread
/// the driver awaits runs with no host frame below it, so a resume may
/// be made only where the store runs no guest code, and a trampoline
/// that has to resume a thread from inside a guest call suspends its
/// own thread first. [`may_resume_here`](Self::may_resume_here) and
/// [`may_suspend_here`](Self::may_suspend_here) answer those two
/// questions.
///
/// A clone is a handle to the same provider, which a caller takes
/// out of the store before calling it with the store beside it.
#[derive(Clone)]
pub enum StoreProvider {
    /// The stack-switching provider.
    StackSwitching(StackSwitchingProvider),
    /// The host-suspension provider.
    HostSuspension(HostSuspensionProvider),
}

impl StoreProvider {
    /// Which provider this is.
    pub fn kind(&self) -> SuspendProviderKind {
        match self {
            Self::StackSwitching(_) => SuspendProviderKind::StackSwitching,
            Self::HostSuspension(_) => SuspendProviderKind::HostSuspension,
        }
    }

    /// Whether a resume of this provider runs the thread once the
    /// driver awaits it, rather than inside the call.
    pub fn resumes_later(&self) -> bool {
        match self {
            Self::StackSwitching(_) => false,
            Self::HostSuspension(_) => true,
        }
    }

    /// Whether a thread may be resumed from the frame that runs now.
    /// The stack-switching provider resumes one anywhere. The
    /// host-suspension provider resumes one only where the store runs
    /// no guest code, which is a turn of a driver.
    pub fn may_resume_here<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        match self {
            Self::StackSwitching(_) => true,
            Self::HostSuspension(provider) => provider.at_rest(store),
        }
    }

    /// Whether a shim called from the host function that runs now may
    /// suspend the stack it runs on, which is what a trampoline that
    /// cannot resume a thread where it stands does instead. Only the
    /// host-suspension provider needs to.
    pub fn may_suspend_here<T: 'static>(&self, store: &mut StoreContext<'_, T>) -> bool {
        match self {
            Self::StackSwitching(_) => false,
            Self::HostSuspension(provider) => provider.may_suspend(store),
        }
    }

    /// Make the shims of the blocking built-ins in `hosts`, one for
    /// each, in the order given. Each entry names the shim's type, the
    /// built-in's try, and its finish.
    pub async fn shims<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        hosts: &[(FuncType, RuntimeFunc, RuntimeFunc)],
    ) -> Result<Vec<RuntimeFunc>> {
        match self {
            Self::StackSwitching(provider) => provider.shims(store, hosts),
            Self::HostSuspension(provider) => provider.shims(store, hosts).await,
        }
    }

    /// Make ready to start threads of the entry types `types`, before
    /// any of them runs. The stack-switching provider makes a start the
    /// first time a thread of its type starts. The host-suspension
    /// provider makes them here, because a thread can start from inside
    /// a guest call, where nothing can wait for an instantiation.
    pub async fn prepare_entries<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        types: &[FuncType],
    ) -> Result<()> {
        match self {
            Self::StackSwitching(_) => Ok(()),
            Self::HostSuspension(provider) => provider.prepare_entries(store, types).await,
        }
    }

    /// Start `entry` as `thread` as the store's flight, from a frame
    /// inside a guest call whose thread suspends for it, which only a
    /// provider that [`resumes_later`](Self::resumes_later) does. The
    /// stack-switching provider starts a thread in place, anywhere.
    pub fn defer_start<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        thread: ThreadId,
        entry: &RuntimeFunc,
        ty: &FuncType,
        args: &[RuntimeVal],
    ) -> Result<EntryStatus> {
        match self {
            Self::StackSwitching(provider) => provider.start(store, thread, entry, ty, args),
            Self::HostSuspension(provider) => provider.defer_start(store, thread, entry, ty, args),
        }
    }

    /// Run the store's flight, the start or the resume a turn left for
    /// the driver, until the thread stops. Only the host-suspension
    /// provider leaves one.
    pub async fn fly<T: 'static>(&self, store: &mut StoreContext<'_, T>) {
        match self {
            Self::StackSwitching(_) => {}
            Self::HostSuspension(provider) => provider.fly(store).await,
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
            Self::HostSuspension(provider) => provider.start(store, thread, entry, ty, args),
        }
    }

    fn resume(&self, store: &mut StoreContext<'_, T>, thread: ThreadId) -> Result<EntryStatus> {
        match self {
            Self::StackSwitching(provider) => provider.resume(store, thread),
            Self::HostSuspension(provider) => provider.resume(store, thread),
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
            Self::HostSuspension(provider) => provider.poll_stop(store, thread, waker),
        }
    }
}
