// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A resumable call that runs.

use core::fmt;

use crate::call::ResumableCall;
use crate::capability::Capability;
use crate::contract::BackendResumption;
use crate::error::{Error, Result};
use crate::internal::StoreContextMutInternal;
use crate::store::{AsContextMut, StoreId};
use crate::values::Val;

/// A resumable call that runs: started with
/// [`Func::start_resumable`](crate::Func::start_resumable) or resumed with
/// [`SuspendedCall::start_resume`](crate::SuspendedCall::start_resume), and
/// not yet seen to stop.
///
/// The handle does not hold its store. It borrows the store only while
/// [`stop`](Self::stop) waits. A wait whose future drops leaves the call
/// with the handle, and a later wait takes it up, so the host can stop
/// waiting for a call without losing it.
///
/// Where the handle drops before a wait saw the call stop, the call stops
/// the next time it would reach its store and never runs again, unless the
/// store dropped first.
pub struct Resumption {
    store: StoreId,
    inner: Option<Box<dyn BackendResumption>>,
}

impl Resumption {
    /// The call `inner` that runs in the store `store`.
    ///
    /// A backend makes one when it starts or resumes a resumable call.
    pub fn new(store: StoreId, inner: Box<dyn BackendResumption>) -> Self {
        Self {
            store,
            inner: Some(inner),
        }
    }

    /// Waits for the call to stop, and answers how it ended: as
    /// [`ResumableCall::Finished`], with its results in `results`, or as
    /// [`ResumableCall::Suspended`], where a suspending host function
    /// answered "not yet".
    ///
    /// `store` is the store the call runs in. A call waited for with
    /// another store is [`Error::WrongStore`], and the handle keeps it.
    ///
    /// Where the future drops before the call stops, the host has the store
    /// back, and the handle keeps the call. A backend that runs the call on
    /// a microtask lets it run on until it would next reach the store, where
    /// it waits without reaching it. The next wait grants it the store again,
    /// and it runs on from where it waited. A backend that runs the call
    /// synchronously has seen it stop before the first wait.
    ///
    /// Once a wait answered, with the stop or with an error, the call is
    /// spent, and a later wait is [`Error::Backend`].
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`].
    #[tracing::instrument(level = "trace", name = "core call stop", skip_all)]
    pub async fn stop(
        &mut self,
        mut store: impl AsContextMut,
        results: &mut [Val],
    ) -> Result<ResumableCall> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        if backend.id() != self.store {
            return Err(Error::WrongStore);
        }
        let inner = self.inner.as_mut().ok_or_else(|| Error::Backend {
            message: "the resumable call already stopped".to_owned(),
        })?;
        let outcome = backend.stop_resumption(inner.as_mut(), results).await;
        self.inner = None;
        outcome
    }
}

impl fmt::Debug for Resumption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resumption")
            .field("store", &self.store)
            .field("stopped", &self.inner.is_none())
            .finish_non_exhaustive()
    }
}
