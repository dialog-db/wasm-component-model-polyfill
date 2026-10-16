// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-import host-function payload stored on a [`LinkerInstance`].
//!
//! `HostFunc<T>` is the runtime carrier for what a developer
//! registers through any of the four function entries a
//! [`LinkerInstance`] offers. Two of them register a synchronous host
//! function: [`LinkerInstance::func_new`] (untyped, takes `Val`
//! slices) and [`LinkerInstance::func_wrap`] (typed, statically
//! shaped). Two register a host `async` function, whose one call
//! produces a future the store owns and polls:
//! [`LinkerInstance::func_new_concurrent`] (untyped) and
//! [`LinkerInstance::func_wrap_concurrent`] (typed). Every path
//! converges on this single type, so the host-trampoline code in
//! [`crate::executor::instantiate`] dispatches against one shape:
//! the signature the resolver checks, and the [`HostFuncKind`] that
//! says which of the two forms the registration is and carries that
//! form's body.
//!
//! [`LinkerInstance`]: super::LinkerInstance
//! [`LinkerInstance::func_new`]: super::LinkerInstance::func_new
//! [`LinkerInstance::func_wrap`]: super::LinkerInstance::func_wrap
//! [`LinkerInstance::func_new_concurrent`]:
//!     super::LinkerInstance::func_new_concurrent
//! [`LinkerInstance::func_wrap_concurrent`]:
//!     super::LinkerInstance::func_wrap_concurrent

use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use std::sync::Arc;

use crate::abi::context::BoundaryContext;
use crate::component::FunctionType;
use crate::concurrency::Accessor;
use crate::error::Result;
use crate::runtime_layer::Val as RuntimeVal;
use crate::store::StoreData;
use crate::typed::{Lower, TypeCheckError, lower_results};
use crate::types::ResourceType;
use crate::value::Val;

use super::host_call::HostCall;
use super::host_func_kind::HostFuncKind;

/// One registered host function inside an [`crate::LinkerInstance`].
///
/// Wraps the body the registration carries and what the resolver
/// checks it against the import with: the [`FunctionType`] the
/// developer named (untyped), or the type check of the Rust types the
/// closure takes and returns (typed). Every registration path produces
/// the same `HostFunc` so the trampoline dispatcher only sees one
/// shape, and the body sits behind a [`HostFuncKind`] that records
/// whether the registration is synchronous or concurrent.
pub struct HostFunc<T: 'static> {
    /// What the resolver checks the import's declared type against at
    /// link time.
    pub signature: HostSignature,
    /// Which of the two registration forms this is, and the body
    /// that form carries. The link rule and the trampoline read the
    /// kind: one to hold an async-typed import to a concurrent
    /// registration, the other to know whether a call runs a closure
    /// to completion or starts a host task.
    pub kind: HostFuncKind<T>,
}

/// What the resolver checks an import's declared function type
/// against, for one registration.
pub enum HostSignature {
    /// The function type an untyped registration names, compared with
    /// the declared one structurally.
    Declared(FunctionType),
    /// The type check of a typed registration's parameter and result
    /// lists, which walks the declared type with the Rust types.
    Typed(Arc<HostTypeCheck>),
}

/// The type check a typed registration carries: it compares the
/// registration's Rust lists with a declared function type, reading
/// each resource type the type declares as the identity the second
/// argument gives it.
pub type HostTypeCheck = dyn Fn(
        &FunctionType,
        &dyn Fn(ResourceType) -> ResourceType,
    ) -> core::result::Result<(), TypeCheckError>
    + Send
    + Sync
    + 'static;

impl Clone for HostSignature {
    fn clone(&self) -> Self {
        match self {
            Self::Declared(ty) => Self::Declared(ty.clone()),
            Self::Typed(check) => Self::Typed(check.clone()),
        }
    }
}

/// How the arguments of one call reach a registration's body.
pub enum HostArgs {
    /// One [`Val`] per parameter, for an untyped registration.
    Vals(Vec<Val>),
    /// The registration's own Rust parameter tuple, lifted straight
    /// out of the guest, for a typed one.
    Typed(Box<dyn Any + Send>),
}

/// What one call of a registration answers with, ready to cross into
/// the guest.
pub enum HostResults<T: 'static> {
    /// One [`Val`] when the type declares a result and none otherwise,
    /// from an untyped registration.
    Vals(Vec<Val>),
    /// A typed registration's result tuple, which lowers itself
    /// straight into the guest.
    Typed(Box<dyn TypedResults<T>>),
}

/// A typed registration's result tuple, as the trampoline holds it
/// until it crosses into the guest.
pub trait TypedResults<T: 'static>: Send {
    /// Lower the results as the result list of `ty`: into the return
    /// area the guest passed, or into the flat result values of the
    /// call.
    fn lower(
        self: Box<Self>,
        cx: &mut BoundaryContext<'_, StoreData<T>>,
        ty: &FunctionType,
        return_area: Option<usize>,
        flat: &mut [RuntimeVal],
    ) -> Result<()>;
}

impl<T: 'static, R: Lower + 'static> TypedResults<T> for R {
    fn lower(
        self: Box<Self>,
        cx: &mut BoundaryContext<'_, StoreData<T>>,
        ty: &FunctionType,
        return_area: Option<usize>,
        flat: &mut [RuntimeVal],
    ) -> Result<()> {
        lower_results(cx, ty, &*self, return_area, flat)
    }
}

/// How a registration's arguments come out of the guest.
pub enum HostParams<T: 'static> {
    /// One [`Val`] per parameter, for an untyped registration.
    Vals,
    /// The registration's Rust parameter tuple, lifted straight out of
    /// the guest by this function, for a typed one.
    Typed(Arc<LiftParams<T>>),
}

/// The lift of a typed registration's parameter tuple out of the core
/// arguments of one call, from the cursor: the flat values, or, when
/// the flag is set, the address of the spilled tuple.
pub type LiftParams<T> = dyn for<'a> Fn(
        &mut BoundaryContext<'a, StoreData<T>>,
        &FunctionType,
        &[RuntimeVal],
        &mut usize,
        bool,
    ) -> Result<Box<dyn Any + Send>>
    + Send
    + Sync
    + 'static;

impl<T: 'static> Clone for HostParams<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Vals => Self::Vals,
            Self::Typed(lift) => Self::Typed(lift.clone()),
        }
    }
}

/// The closure a synchronous registration holds.
///
/// The closure takes a [`HostCall`] (the host's view of the call) and
/// the call's arguments, and answers with the call's results.
pub type HostFuncBody<T> =
    dyn for<'a> Fn(HostCall<'a, T>, HostArgs) -> Result<HostResults<T>> + Send + Sync + 'static;

/// The boxed future one call of a concurrent registration produces,
/// with the `Send` bound the native target puts on everything a
/// store holds.
///
/// The future is what the store owns as a host task and polls until
/// it completes, so it is `'static` and borrows neither the store nor
/// the call that started it.
#[cfg(not(target_arch = "wasm32"))]
pub type HostFuncFuture<T> = Pin<Box<dyn Future<Output = Result<HostResults<T>>> + Send + 'static>>;

/// The boxed future one call of a concurrent registration produces.
/// The browser drops the `Send` bound, because a future that awaits a
/// JavaScript promise is not `Send`: see [`HostFuture`].
///
/// [`HostFuture`]: crate::HostFuture
#[cfg(target_arch = "wasm32")]
pub type HostFuncFuture<T> = Pin<Box<dyn Future<Output = Result<HostResults<T>>> + 'static>>;

/// The closure a concurrent registration holds.
///
/// The closure takes the accessor of the store the call runs against
/// and the call's arguments, owned, and answers with the future of
/// that one call. It is the registration's body in the sense the
/// store's host tasks use: the trampoline calls it to start the call,
/// hands the future to the store, and returns to the guest.
pub type ConcurrentHostFuncBody<T> =
    dyn Fn(&Accessor<T>, HostArgs) -> HostFuncFuture<T> + Send + Sync + 'static;

/// A registration is cloned by its signature and by a handle on its
/// body, whatever the host data is: nothing of `T` is held by value,
/// so the clone asks nothing of it.
impl<T: 'static> Clone for HostFunc<T> {
    fn clone(&self) -> Self {
        Self {
            signature: self.signature.clone(),
            kind: self.kind.clone(),
        }
    }
}
