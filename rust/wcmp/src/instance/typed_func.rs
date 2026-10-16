// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A typed handle for invoking a component export with native Rust
//! values.
//!
//! `TypedFunc<P, R>` is the typed counterpart to [`Func`]: the
//! parameter list `P` and the result list `R`, both tuples, are checked
//! once, when the handle is acquired through [`Func::typed`], against
//! the export's declared component-level signature. At call time the
//! Rust values cross the canonical-ABI boundary directly: neither the
//! caller nor the polyfill builds a [`Val`] for them, at any depth.
//!
//! The lists are constrained by Wasmtime's traits: [`Lower`] and
//! [`Lift`] for the values, and [`ComponentNamedList`] for a tuple that
//! is a function's list.
//!
//! [`Val`]: crate::Val
//! [`Lower`]: crate::Lower
//! [`Lift`]: crate::Lift
//! [`ComponentNamedList`]: crate::ComponentNamedList

use core::marker::PhantomData;

use crate::concurrency::Accessor;
use crate::error::{Error, Result, TypeMismatch, TypeMismatchPosition};
use crate::internal::{
    ErrorInternal, FuncInternal, ResourceTypeInternal, TypeCheckErrorInternal, TypedFuncInternal,
};
use crate::store::Store;
use crate::typed::{ComponentNamedList, Lift, Lower, typecheck_function};
use crate::types::ResourceType;

use super::func::Func;
use super::typed_call::TypedCall;

/// A statically-typed handle for invoking one component export.
///
/// `TypedFunc<P, R>` is acquired from [`Func::typed`]; the conversion
/// checks that the export's declared signature matches `P` and `R` and
/// surfaces a structured [`Error::TypeMismatch`] on mismatch. Calling
/// the handle takes a tuple of the parameters and returns a tuple of
/// the results, as Wasmtime's does: a function with one result returns
/// a tuple of one value. The
/// call runs the same task, scheduler item, and driver as
/// [`Func::call`], so heap-allocating valtypes drive the same
/// `cabi_realloc` / `post-return` round-trip the untyped path
/// observes; only the lowering of the arguments and the lifting of
/// the result differ, and they go straight between the Rust values
/// and the guest rather than through [`Val`].
///
/// [`Func::typed`]: super::Func::typed
/// [`Func::call`]: super::Func::call
/// [`Val`]: crate::Val
pub struct TypedFunc<P, R> {
    inner: Func,
    _phantom: PhantomData<fn(P) -> R>,
}

impl<P, R> core::fmt::Debug for TypedFunc<P, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TypedFunc")
            .field("name", &self.inner.name())
            .field("signature", &self.inner.signature())
            .finish()
    }
}

impl<P, R> TypedFuncInternal<P, R> for TypedFunc<P, R> {
    fn from_checked(inner: Func) -> TypedFunc<P, R> {
        TypedFunc {
            inner,
            _phantom: PhantomData,
        }
    }
}

impl<P, R> TypedFunc<P, R>
where
    P: ComponentNamedList + Lower + 'static,
    R: ComponentNamedList + Lift + 'static,
{
    /// Invoke the export with native Rust values.
    ///
    /// `args` is the parameter tuple `P`; the return is the result
    /// tuple `R` the export produces. Compound
    /// argument and return shapes drive the same canonical-ABI
    /// round-trip — `cabi_realloc` for heap-allocating values and
    /// `post-return` after the result is observed — that the
    /// untyped [`Func::call`] path observes.
    ///
    /// The values go straight between Rust and the guest. A `String`
    /// or a `Vec` of numbers is written into guest memory in one
    /// access and read back out in one, and a `Vec<u8>` or a UTF-8
    /// string is not copied on the host beyond that, so a payload of
    /// hundreds of megabytes costs its own size once per direction. A
    /// list of any other type lifts and lowers each element through
    /// its own type, out of one read of the list and into one write.
    /// The result of an export lifted `async` arrives through
    /// `task.return`, which lifts it the same way.
    ///
    /// `T` is the host-data type of the [`Store`] the export's
    /// owning [`Instance`] was created in.
    ///
    /// # Where a trap surfaces
    ///
    /// The first trap ends the driver that is polling the store, with
    /// that trap, and poisons the store in the same step, as
    /// [`Func::call`] states. While this call runs turns, it is that
    /// driver: a trap in its own task ends it, and so does a trap in
    /// work another task left after it resolved, or a host `async`
    /// function whose future fails, when one of its turns meets it.
    /// A trap in this call's task after the call has returned its
    /// result is never reported here: the driver whose turn meets it
    /// reports it, and every driver after it fails with the
    /// cannot-enter cause.
    ///
    /// [`Func::call`]: super::Func::call
    /// [`Store`]: crate::Store
    /// [`Instance`]: super::Instance
    /// [`Val`]: crate::Val
    pub async fn call<T: 'static>(&self, store: &mut Store<T>, args: P) -> Result<R> {
        self.inner
            .call_values(store, TypedCall::<P, R>::new(args))
            .await
    }

    /// Invoke the export from inside a poll of the store, with
    /// native Rust values.
    ///
    /// This is the typed counterpart to [`Func::call_concurrent`]:
    /// `args` is the parameter tuple `P` and the return is the result
    /// tuple `R`, and everything else is that entry's.
    /// The call reaches the store through the accessor a
    /// `run_concurrent` closure or a host `async` function's body
    /// holds, creates the export's task, and queues its start behind
    /// the entry gate of the export's instance. The returned future
    /// resolves when the task's result is set. It is spawn-like:
    /// dropping it cancels nothing, the task progresses only while a
    /// driver runs turns, and a store that goes idle with the task
    /// unresolved leaves the future pending rather than failing it.
    /// It registers the waker it is polled with and is woken when
    /// the task resolves or fails, so several such calls can be
    /// awaited together through a waker-gated combinator.
    ///
    /// `T` is the host-data type of the [`Store`] the export's
    /// owning [`Instance`] was created in.
    ///
    /// [`Func::call_concurrent`]: super::Func::call_concurrent
    /// [`Store`]: crate::Store
    /// [`Instance`]: super::Instance
    pub async fn call_concurrent<T: 'static>(&self, accessor: &Accessor<T>, args: P) -> Result<R> {
        self.inner
            .call_concurrent_values(accessor, TypedCall::<P, R>::new(args))
            .await
    }
}

impl Func {
    /// Convert this untyped function handle into a typed one whose
    /// parameter tuple `P` and result tuple `R` are checked against the
    /// export's declared component-level signature.
    ///
    /// The check walks the declared types and the Rust types together,
    /// in Wasmtime's order: the count of parameters and results, then
    /// each one in turn, and inside a record, a variant, an enum, or a
    /// flags type, the count of its parts and then each part's name
    /// and type. It has no subtyping. A resource handle matches by the
    /// identity of its resource type. The check runs here and never on
    /// a call.
    ///
    /// Returns [`Error::TypeMismatch`] when the types differ. Its
    /// position is [`TypeMismatchPosition::TypedConversion`], whose
    /// path names the first part that differs, for example
    /// "result 0, then list element". The `expected` rendering carries
    /// the declared type at the end of the path and the `actual` one
    /// the Rust type there.
    pub fn typed<P, R>(self) -> Result<TypedFunc<P, R>>
    where
        P: ComponentNamedList + Lower,
        R: ComponentNamedList + Lift,
    {
        // The parameter names and the `async` effect take no part: a
        // Rust tuple names neither, and a callback export and a
        // synchronous export of the same shape take the same values.
        // A resource type the export declares is read as the identity
        // the instantiation gave it: the host registration that
        // satisfied its import, or the instance that defines it.
        let resources = self
            .abi_state()
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?
            .resource_tables
            .clone();
        let resolve = |declared: ResourceType| {
            declared
                .resource_index()
                .and_then(|index| {
                    resources
                        .iter()
                        .flatten()
                        .find(|table| table.resource_index == index)
                })
                .map_or(declared, |table| table.type_id)
        };
        typecheck_function::<P, R>(self.signature(), &resolve).map_err(|error| {
            let (path, expected, actual) = error.into_parts();
            Error::from(TypeMismatch {
                position: TypeMismatchPosition::TypedConversion {
                    export: self.name().to_owned(),
                    path,
                },
                expected,
                actual,
            })
        })?;
        Ok(TypedFunc::from_checked(self))
    }
}
