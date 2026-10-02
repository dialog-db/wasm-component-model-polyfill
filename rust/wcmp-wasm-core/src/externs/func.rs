// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A function.

use core::task::Poll;

use crate::call::{ResumableCall, Resumption};
use crate::capability::Capability;
use crate::checks;
use crate::contract::HostFunc;
use crate::error::Result;
use crate::internal::{CallerInternal, StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut, Caller};
use crate::types::FuncType;
use crate::values::Val;

handle! {
    /// A function: an export of an instance, a host function, or a
    /// function reference a guest handed out.
    Func
}

impl Func {
    /// A host function of type `ty` in `store`, whose body is `func`.
    ///
    /// The body is `Fn`, not `FnMut`: the engine can enter it again while
    /// an earlier call of it runs, at any depth. Each call has its own
    /// arguments and its own slot for each result. The body receives a
    /// [`Caller`], which reaches the store. An error from the body traps
    /// the guest with [`TrapKind::Host`](crate::TrapKind::Host), carrying
    /// the error unchanged, and no guest can catch the trap.
    ///
    /// The body writes a value of the function's type to each result
    /// slot. A result of another type traps the guest with
    /// [`TrapKind::Host`](crate::TrapKind::Host) too, and the error it
    /// carries is [`Error::TypeMismatch`](crate::Error::TypeMismatch), which
    /// the host finds with `downcast_ref::<Error>()`.
    ///
    /// A type that needs a capability the backend lacks, such as a
    /// parameter of a GC reference type, is
    /// [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn new<T: 'static>(
        mut store: impl AsContextMut<Data = T>,
        ty: FuncType,
        func: impl Fn(Caller<'_, T>, &[Val], &mut [Val]) -> anyhow::Result<()> + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut store = store.as_context_mut();
        checks::func_type(store.engine().capabilities(), &ty)?;
        let body = HostFunc::new(move |store, params, results| {
            func(Caller::from_backend(store), params, results)
        });
        store.backend_mut().func_new(ty, body)
    }

    /// A suspending host function of type `ty` in `store`, whose body is
    /// `func`.
    ///
    /// The body answers [`Poll::Ready`] once it wrote its results, or
    /// [`Poll::Pending`]: "not yet". Inside a call made with
    /// [`call_resumable`](Func::call_resumable), "not yet" suspends the call
    /// when WebAssembly frames alone lie between the start of the call and
    /// this function. Anywhere else, the call traps. Otherwise the body
    /// holds the rules of [`Func::new`].
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn new_suspending<T: 'static>(
        mut store: impl AsContextMut<Data = T>,
        ty: FuncType,
        func: impl Fn(Caller<'_, T>, &[Val], &mut [Val]) -> anyhow::Result<Poll<()>>
        + Send
        + Sync
        + 'static,
    ) -> Result<Self> {
        let mut store = store.as_context_mut();
        let capabilities = store.engine().capabilities();
        capabilities.require(Capability::HostSuspension)?;
        checks::func_type(capabilities, &ty)?;
        let body = HostFunc::suspending(move |store, params, results| {
            func(Caller::from_backend(store), params, results)
        });
        store.backend_mut().func_new(ty, body)
    }

    /// The type of the function, or `None` where the engine does not know
    /// it, as the browser does not for a function reference a guest handed
    /// out.
    pub fn ty(&self, store: impl AsContext) -> Result<Option<FuncType>> {
        let store = store.as_context();
        let backend = store.backend();
        checks::same_store(backend, *self)?;
        backend.func_ty(*self)
    }

    /// Calls the function with `params`, and writes its results to
    /// `results`, one slot for each result.
    ///
    /// The call is untyped: a value or a number of values that does not
    /// match the type of the function is
    /// [`Error::TypeMismatch`](crate::Error::TypeMismatch). A trap is
    /// [`Error::Trap`](crate::Error::Trap).
    pub fn call(
        &self,
        mut store: impl AsContextMut,
        params: &[Val],
        results: &mut [Val],
    ) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::values_in_store(backend, params)?;
        backend.func_call(*self, params, results)
    }

    /// Calls the function as a resumable call.
    ///
    /// The call ends as [`ResumableCall::Finished`], with its results in
    /// `results`, or as [`ResumableCall::Suspended`], where a suspending
    /// host function answered "not yet". The host resumes a suspended call
    /// later, with the results of that host function.
    ///
    /// The call is asynchronous because the browser delivers its end
    /// through a promise. The first stretch of the call runs on the
    /// future's first poll, so a call that suspends there ends on that
    /// poll, even inside a host function, which cannot wait. A call that
    /// finishes there ends on that poll too, unless the browser's backend
    /// cannot name the function's type in a generated module. A call that
    /// traps, or finishes after it suspended, can end only once the browser
    /// settles its promise.
    /// Where the future drops before the call ends, the host has the store
    /// back, as for [`SuspendedCall::resume`](crate::SuspendedCall::resume).
    /// [`start_resumable`](Self::start_resumable) keeps the call instead.
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub async fn call_resumable(
        &self,
        mut store: impl AsContextMut,
        params: &[Val],
        results: &mut [Val],
    ) -> Result<ResumableCall> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::values_in_store(backend, params)?;
        backend.func_call_resumable(*self, params, results).await
    }

    /// Starts the function as a resumable call, and answers the call as a
    /// [`Resumption`], whose [`stop`](Resumption::stop) waits for its end or
    /// its suspension.
    ///
    /// This is [`call_resumable`](Self::call_resumable) in two steps. The
    /// first stretch of the call runs here, so a call that suspends there
    /// has stopped before the first wait. The handle keeps the call where a
    /// wait's future drops, and a later wait takes it up.
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn start_resumable(
        &self,
        mut store: impl AsContextMut,
        params: &[Val],
    ) -> Result<Resumption> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::values_in_store(backend, params)?;
        backend.func_start_resumable(*self, params)
    }
}
