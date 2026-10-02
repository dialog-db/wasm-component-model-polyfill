// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A resumable call that waits.

use core::fmt;

use crate::call::{ResumableCall, Resumption};
use crate::capability::Capability;
use crate::checks;
use crate::contract::BackendSuspendedCall;
use crate::error::Result;
use crate::internal::StoreContextMutInternal;
use crate::store::{AsContextMut, StoreId};
use crate::values::Val;

/// A resumable call that waits in a suspending host function.
///
/// The handle does not hold its store. It borrows the store only while it
/// resumes. Any number of calls can wait at once in one store, and the host
/// can resume them in any order. When the store drops, its waiting calls
/// drop without a resumption.
pub struct SuspendedCall {
    store: StoreId,
    inner: Box<dyn BackendSuspendedCall>,
}

impl SuspendedCall {
    /// The waiting call `inner` of the store `store`.
    ///
    /// A backend makes one when a suspending host function answers "not
    /// yet".
    pub fn new(store: StoreId, inner: Box<dyn BackendSuspendedCall>) -> Self {
        Self { store, inner }
    }

    /// Resumes the call with `import_results`, the results of the
    /// suspending host function, and runs it to its next suspension or its
    /// end. At its end, the results of the call are in `results`.
    ///
    /// The resumption is asynchronous, because the browser runs the rest of
    /// the call on a microtask after the resumption starts. `store` is the
    /// store the call waits in, or the [`Caller`](crate::Caller) of a host
    /// function that runs in it. A call resumed with a store other than its
    /// own is [`Error::WrongStore`](crate::Error::WrongStore).
    ///
    /// Where the future drops before the resumption ends, the host has the
    /// store back, and the call is lost. A backend that runs the rest of the
    /// call on a microtask lets it run on, but the call no longer reaches the
    /// store: it stops the next time it would, never to run again, and
    /// nothing waits for its end. [`start_resume`](Self::start_resume) keeps
    /// the call instead. Only where the
    /// store drops first does the call run to its next suspension or its
    /// end with the state of the store, since nothing else can reach it
    /// then.
    ///
    /// A host function cannot wait, and a backend that runs the rest of
    /// the call on a microtask runs it only after the host function
    /// returned. So there, a resumption from inside a host function fails
    /// with [`Error::Backend`](crate::Error::Backend), and the call drops
    /// without a resumption. Resume a call from the store itself.
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub async fn resume(
        self,
        mut store: impl AsContextMut,
        import_results: &[Val],
        results: &mut [Val],
    ) -> Result<ResumableCall> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        if backend.id() != self.store {
            return Err(crate::Error::WrongStore);
        }
        checks::values_in_store(backend, import_results)?;
        backend
            .resume_call(self.inner, import_results, results)
            .await
    }

    /// Resumes the call with `import_results`, the results of the
    /// suspending host function, and answers the call as a [`Resumption`],
    /// whose [`stop`](Resumption::stop) waits for its next suspension or its
    /// end.
    ///
    /// This is [`resume`](Self::resume) in two steps. The handle keeps the
    /// call where a wait's future drops, and a later wait takes it up. A
    /// call resumed with a store other than its own is
    /// [`Error::WrongStore`](crate::Error::WrongStore), and a resumption from
    /// inside a host function fails as for [`resume`](Self::resume).
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn start_resume(
        self,
        mut store: impl AsContextMut,
        import_results: &[Val],
    ) -> Result<Resumption> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        if backend.id() != self.store {
            return Err(crate::Error::WrongStore);
        }
        checks::values_in_store(backend, import_results)?;
        backend.start_resume(self.inner, import_results)
    }
}

impl fmt::Debug for SuspendedCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuspendedCall")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}
