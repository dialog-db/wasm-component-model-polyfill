// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A borrowed view onto one interface's registration in a [`Linker`].
//!
//! [`Linker`]: super::Linker

use core::any::Any;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::component::FunctionType;
use crate::concurrency::{Accessor, HostFuture};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, LinkError, Result};
use crate::internal::ErrorInternal;
use crate::module::Module;
use crate::store::StoreContext;
use crate::typed::{ComponentNamedList, Lift, Lower, lift_parameters, typecheck_function};
use crate::types::{ResourceType, ValueType};
use crate::value::Val;

use super::host_args::HostArgs;
use super::host_call::HostCall;
use super::host_func::{HostFunc, HostFuncFuture};
use super::host_func_kind::HostFuncKind;
use super::host_params::HostParams;
use super::host_results::HostResults;
use super::host_signature::HostSignature;
use super::registration::InstanceRegistration;
use super::resource_registration::ResourceRegistration;
use crate::internal::LinkerInstanceInternal;

/// A borrowed view onto one interface's worth of host items inside a
/// [`Linker`].
///
/// `LinkerInstance` is the addressing surface for "the host items
/// that satisfy *this* interface": it is obtained from
/// [`Linker::instance`] and carries the back-reference the linker
/// uses to look up or insert a per-interface registration entry.
/// Four function registration modes are exposed, two for a
/// synchronous host function and two for a host `async` function:
///
/// - [`Self::func_new`] (untyped): take an explicit
///   [`FunctionType`] and a closure over [`HostCall`],
///   `&[Val]`, `&mut [Val]`.
/// - [`Self::func_wrap`] (typed): take a closure with statically-
///   typed Rust arguments and return; the polyfill derives the
///   declared signature from the closure's generic parameters.
/// - [`Self::func_new_concurrent`] (untyped): take an explicit
///   [`FunctionType`] and a closure over [`Accessor`] and an owned
///   `Vec<Val>` that answers with the future of one call.
/// - [`Self::func_wrap_concurrent`] (typed): take the same closure
///   with statically-typed Rust arguments and return, and derive the
///   declared signature as [`Self::func_wrap`] does.
///
/// Every mode converges on a single [`HostFunc<T>`] that the linker
/// stores against the item's name, then the resolver's link-time
/// type check consults when matching the registration against the
/// component's declared import. The registration records which of
/// the two forms it is, and that is what the link rule and the
/// trampoline read.
///
/// A third mode, [`Self::resource`], registers a host resource type
/// by its identity, with a synchronous destructor closure, under the
/// name the component imports it by. A fourth, [`Self::module`], registers a core
/// [`Module`] for a module-typed import.
///
/// Every mode refuses a name the view's entry already holds an item
/// under, whatever kind of item that is, and answers with
/// [`Error::Link`]. [`Linker::allow_shadowing`] turns the refusal
/// off for the views a linker hands out, and the view carries what
/// the linker was told. [`Self::instance`] is the exception, because
/// it addresses rather than registers: it answers with the entry
/// already there.
///
/// [`Linker`]: super::Linker
/// [`Linker::instance`]: super::Linker::instance
/// [`Linker::allow_shadowing`]: super::Linker::allow_shadowing
/// [`Error::Link`]: crate::Error::Link
pub struct LinkerInstance<'a, T: 'static> {
    /// The owned registration this view borrows. Held mutably so
    /// later registration methods can populate it without further
    /// linker access.
    registration: &'a mut InstanceRegistration<T>,
    /// Whether a registration may take a name the entry already
    /// holds an item under. Copied from the linker when the view is
    /// made, and carried down to the views [`Self::instance`]
    /// returns.
    allow_shadowing: bool,
    /// `T` participates only as the host-data type the registration
    /// carries; capture invariance explicitly so the parameter does
    /// not appear unused.
    _phantom: PhantomData<fn(T) -> T>,
}

impl<'a, T: 'static> LinkerInstanceInternal<'a, T> for LinkerInstance<'a, T> {
    fn new(registration: &'a mut InstanceRegistration<T>, allow_shadowing: bool) -> Self {
        LinkerInstance {
            registration,
            allow_shadowing,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T: 'static> LinkerInstance<'a, T> {
    /// Address (creating if absent) the nested registration for an
    /// instance item under `name`: a plain-named instance import,
    /// `(import "name" (instance …))`, on the root view a
    /// [`Linker::root`] returns, or an instance exported by the
    /// instance this view addresses, at any depth. A component that
    /// imports `a` whose `b` instance exports `f` links against
    /// `root().instance("a").instance("b").func_wrap("f", …)`, as it
    /// does in Wasmtime.
    ///
    /// This addresses an entry rather than registering an item, so
    /// calling it twice under one name answers with the same nested
    /// entry both times rather than refusing the second call. The
    /// name is taken all the same: a function, resource, or module
    /// registered under it afterwards is the duplicate that fails.
    ///
    /// [`Linker::root`]: super::Linker::root
    pub fn instance(&mut self, name: impl Into<String>) -> LinkerInstance<'_, T> {
        let allow_shadowing = self.allow_shadowing;
        let entry = self.registration.instances.entry(name.into()).or_default();
        LinkerInstance::new(entry, allow_shadowing)
    }

    /// Refuse `name` when the entry this view borrows already holds
    /// an item under it and the linker was not told to allow
    /// shadowing.
    ///
    /// Called by every registration mode before it inserts, so the
    /// name a registration takes is the name it keeps.
    fn claim(&self, name: &str) -> Result<()> {
        if self.allow_shadowing || self.registration.kind_of(name).is_none() {
            return Ok(());
        }
        Err(LinkError::DuplicateRegistration {
            name: name.to_owned(),
        }
        .into())
    }

    /// Register an *untyped* host function. The caller supplies the
    /// declared [`FunctionType`] explicitly; the closure takes the
    /// [`HostCall`] context, a slice of polyfill [`Val`]
    /// arguments, and a slice the implementation fills with the
    /// returned values. The result slice's length is one if the
    /// signature declares a result, zero otherwise.
    ///
    /// The registration is stored against the given item-name on
    /// the underlying [`InstanceRegistration`]; the linker's
    /// resolver checks the declared signature against the
    /// component's import at link time.
    ///
    /// Fails with [`Error::Link`] when the entry this view addresses
    /// already holds an item under `name` and the linker was not
    /// told to allow shadowing.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn func_new(
        &mut self,
        name: impl Into<String>,
        ty: FunctionType,
        func: impl for<'c> Fn(HostCall<'c, T>, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        let name = name.into();
        self.claim(&name)?;
        // The result slice is sized from the declared type, which the
        // link-time check holds the import's to.
        let arity = usize::from(ty.result.is_some());
        let body = move |call: HostCall<'_, T>, args: HostArgs| {
            let args = untyped_args(args)?;
            let mut results = vec![Val::Bool(false); arity];
            func(call, &args, &mut results)?;
            Ok(HostResults::Vals(results))
        };
        self.registration.funcs.insert(
            name,
            HostFunc {
                signature: HostSignature::Declared(ty),
                kind: HostFuncKind::Synchronous(Arc::new(body), HostParams::Vals),
            },
        );
        Ok(())
    }

    /// Register a host resource type under `name`, with its identity
    /// `ty` and a synchronous destructor, as Wasmtime's
    /// `LinkerInstance::resource` does.
    ///
    /// The destructor runs when a guest drops an owned handle of the
    /// type. It receives a context for the store, which reaches the
    /// host data, and the resource's rep.
    ///
    /// The identity is what tells the resource type from every other.
    /// A host that uses one resource type in two interfaces registers
    /// the same identity in each, usually [`ResourceType::host`] of
    /// one Rust type: a handle that a guest has through one of them
    /// lowers through the other, and the resolver accepts a component
    /// that declares the two equal, so give the registrations one
    /// destructor. A guest's drop runs the destructor registered under
    /// the interface the guest imported the type through. The host's
    /// release of a [`ResourceAny`](crate::ResourceAny) runs the
    /// destructor of the first instantiation into the store that
    /// imported the type.
    ///
    /// ```rust
    /// use wcmp::{Engine, InterfaceIdentifier, Linker, ResourceType};
    ///
    /// struct File;
    ///
    /// # fn main() -> wcmp::Result<()> {
    /// # let engine = Engine::with_backend(wcmp_wasm_core_wasmtime::Wasmtime::new().unwrap())?;
    /// let mut linker: Linker<Vec<u32>> = Linker::new(&engine);
    /// let id: InterfaceIdentifier = "example:files/types".parse().unwrap();
    /// let mut files = linker.instance(&id);
    /// files.resource("file", ResourceType::host::<File>(), |mut store, rep| {
    ///     store.data_mut().push(rep);
    ///     Ok(())
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Fails with [`Error::Link`] when the entry this view addresses
    /// already holds an item under `name` and the linker was not
    /// told to allow shadowing.
    ///
    /// [`ResourceType::host`]: crate::ResourceType::host
    /// [`Error::Link`]: crate::Error::Link
    pub fn resource(
        &mut self,
        name: impl Into<String>,
        ty: ResourceType,
        destructor: impl for<'c> Fn(StoreContext<'c, T>, u32) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        let name = name.into();
        self.claim(&name)?;
        self.registration.resources.insert(
            name,
            ResourceRegistration {
                ty,
                destructor: Arc::new(destructor),
            },
        );
        Ok(())
    }

    /// Register a core module for a module-typed import,
    /// `(import "name" (core module …))` on the root view or an
    /// `(export "name" (core module …))` item of an instance import
    /// on an interface or nested view. The component instantiates
    /// the module itself, with the imports it names, and can
    /// re-export it. At link time the resolver checks that the
    /// module provides every export the import's module type
    /// declares and asks for no import the type does not list.
    ///
    /// Calling `module` twice with the same name fails with
    /// [`Error::Link`], as any other duplicate registration does,
    /// unless the linker was told to allow shadowing.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn module(&mut self, name: impl Into<String>, module: &Module) -> Result<()> {
        let name = name.into();
        self.claim(&name)?;
        self.registration.modules.insert(name, module.clone());
        Ok(())
    }

    /// Register a *typed* host function, as Wasmtime's `func_wrap`
    /// does. The closure takes the call's parameters as one tuple and
    /// answers with its results as one tuple: a function with one
    /// result returns a tuple of one value.
    ///
    /// ```rust
    /// # fn register(linker: &mut wcmp::Linker<()>) -> wcmp::Result<()> {
    /// linker.root().func_wrap("add", |_call, (a, b): (u32, u32)| Ok((a + b,)))?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The values cross directly: the parameters are lifted straight
    /// out of the guest into the tuple, and the results lowered
    /// straight back, with no [`Val`] at any depth. The resolver runs
    /// the type check of the two tuples against the import's declared
    /// type when the linker links a component, and the call trusts
    /// it.
    ///
    /// Fails with [`Error::Link`] when the entry this view addresses
    /// already holds an item under `name` and the linker was not
    /// told to allow shadowing.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn func_wrap<Params, Results, F>(&mut self, name: impl Into<String>, func: F) -> Result<()>
    where
        Params: ComponentNamedList + Lift + 'static,
        Results: ComponentNamedList + Lower + 'static,
        F: for<'c> Fn(HostCall<'c, T>, Params) -> Result<Results> + Send + Sync + 'static,
    {
        let name = name.into();
        self.claim(&name)?;
        let body = move |call: HostCall<'_, T>, args: HostArgs| {
            let results = func(call, typed_args::<Params>(args)?)?;
            Ok(HostResults::Typed(Box::new(results)))
        };
        self.registration.funcs.insert(
            name,
            HostFunc {
                signature: typed_signature::<Params, Results>(),
                kind: HostFuncKind::Synchronous(Arc::new(body), typed_params::<T, Params>()),
            },
        );
        Ok(())
    }

    /// Register an *untyped* host `async` function: one whose call
    /// produces a future rather than a result.
    ///
    /// The closure takes the accessor of the store the call runs
    /// against and the lifted arguments, and answers with the future
    /// of that one call. Both are owned, where the synchronous entry
    /// lends slices, because nothing the future borrows from the call
    /// survives it: the trampoline hands the future to the store and
    /// returns to the guest, so the future outlives the frame the
    /// call was made on.
    ///
    /// The future runs as a host task the store polls. The turns that
    /// follow the call poll it, with the waker of whichever driver is
    /// running the store, until it completes; the store then lowers
    /// its value into the subtask the guest waits on. So the future is
    /// `'static`, and `Send` besides on the native target, which is
    /// what keeps a store `Send`. The browser bound is `'static`
    /// alone, so a future that awaits a JavaScript promise satisfies
    /// it.
    ///
    /// The accessor is a token, not a borrow. It carries the store's
    /// identity, borrows nothing, and reaches the store only inside
    /// the closure [`Accessor::with`] runs, and only while a poll of
    /// that store is running. The call of the registration's closure
    /// is one such poll, and so is every poll of the future it
    /// answers with: the closure reaches the store before the future
    /// exists, and the future — which can clone the token and hold
    /// the clone across its awaits — reaches it again in a later
    /// poll. A value read out of the host data must be cloned out of
    /// the reach, and a reach made where no poll of the store is
    /// running fails rather than lending the store.
    ///
    /// The caller supplies the declared [`FunctionType`] here, as
    /// [`Self::func_new`] does; the resolver checks it against the
    /// component's import at link time. When the future completes, its
    /// vector must hold one value if `ty` declares a result and none
    /// otherwise: a result vector of the wrong length fails the call,
    /// as an untyped synchronous registration's mismatched value does.
    ///
    /// Fails with [`Error::Link`] when the entry this view addresses
    /// already holds an item under `name` and the linker was not
    /// told to allow shadowing.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn func_new_concurrent<F, Fut>(
        &mut self,
        name: impl Into<String>,
        ty: FunctionType,
        func: F,
    ) -> Result<()>
    where
        F: Fn(&Accessor<T>, Vec<Val>) -> Fut + Send + Sync + 'static,
        Fut: HostFuture,
    {
        let name = name.into();
        self.claim(&name)?;
        let declared = ty.result.clone();
        let body = move |accessor: &Accessor<T>, args: HostArgs| -> HostFuncFuture<T> {
            let args = match untyped_args(args) {
                Ok(args) => args,
                Err(error) => return Box::pin(core::future::ready(Err(error))),
            };
            let future = func(accessor, args);
            let declared = declared.clone();
            Box::pin(async move {
                let values = future.await?;
                let expected = usize::from(declared.is_some());
                if values.len() != expected {
                    return Err(result_arity(declared));
                }
                Ok(HostResults::Vals(values))
            })
        };
        self.registration.funcs.insert(
            name,
            HostFunc {
                signature: HostSignature::Declared(ty),
                kind: HostFuncKind::Concurrent(Arc::new(body), HostParams::Vals),
            },
        );
        Ok(())
    }

    /// Register a *typed* host `async` function: one whose call
    /// produces a future rather than a result, as Wasmtime's
    /// `func_wrap_concurrent` does.
    ///
    /// The closure takes the call's parameters as one tuple, as
    /// [`Self::func_wrap`]'s does, and its future answers with the
    /// results as one tuple. The values cross directly, and the
    /// resolver runs the type check of the two tuples against the
    /// import's declared type at link time.
    ///
    /// The closure takes the accessor of the store the call runs
    /// against and the lifted parameters, and answers with the future
    /// of that one call. That future runs as a host task the store
    /// polls: the turns that follow the call poll it, with the waker
    /// of whichever driver is running the store, until it completes,
    /// and the store then lowers its results into the subtask the
    /// guest waits on. So the future is `'static`, and `Send` besides
    /// on the native target, which is what keeps a store `Send`. The
    /// browser bound is `'static` alone, so a future that awaits a
    /// JavaScript promise satisfies it.
    ///
    /// The accessor is a token, not a borrow. It carries the store's
    /// identity, borrows nothing, and reaches the store only inside
    /// the closure [`Accessor::with`] runs, and only while a poll of
    /// that store is running. The call of the registration's closure
    /// is one such poll, and so is every poll of the future it
    /// answers with: a closure that returns an `async` block reaches
    /// the store before the block exists and moves what it read in,
    /// and the block, which can hold a clone of the token across its
    /// awaits, reaches the store again on the other side of an await.
    /// A value read out of the host data must be cloned out of the
    /// reach, and a reach made where no poll of the store is running
    /// fails rather than lending the store.
    ///
    /// Fails with [`Error::Link`] when the entry this view addresses
    /// already holds an item under `name` and the linker was not
    /// told to allow shadowing.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn func_wrap_concurrent<Params, Results, F, Fut>(
        &mut self,
        name: impl Into<String>,
        func: F,
    ) -> Result<()>
    where
        Params: ComponentNamedList + Lift + 'static,
        Results: ComponentNamedList + Lower + 'static,
        F: Fn(&Accessor<T>, Params) -> Fut + Send + Sync + 'static,
        Fut: HostFuture<Result<Results>>,
    {
        let name = name.into();
        self.claim(&name)?;
        let body = move |accessor: &Accessor<T>, args: HostArgs| -> HostFuncFuture<T> {
            // The parameters are lifted before the closure runs, so a
            // failure to take them is what the future answers with
            // rather than something the call can report: the call is
            // under way by the time there is a future at all.
            let params = match typed_args::<Params>(args) {
                Ok(params) => params,
                Err(error) => return Box::pin(core::future::ready(Err(error))),
            };
            let future = func(accessor, params);
            Box::pin(async move { Ok(HostResults::Typed(Box::new(future.await?))) })
        };
        self.registration.funcs.insert(
            name,
            HostFunc {
                signature: typed_signature::<Params, Results>(),
                kind: HostFuncKind::Concurrent(Arc::new(body), typed_params::<T, Params>()),
            },
        );
        Ok(())
    }
}

/// The type check a typed registration over the lists `P` and `R`
/// carries, for the resolver to run against an import's declared type.
fn typed_signature<P: ComponentNamedList, R: ComponentNamedList>() -> HostSignature {
    HostSignature::Typed(Arc::new(|ty, resources| {
        typecheck_function::<P, R>(ty, resources)
    }))
}

/// The lift of a typed registration's parameter tuple `P`.
fn typed_params<T: 'static, P: Lift + 'static>() -> HostParams<T> {
    HostParams::Typed(Arc::new(|cx, ty, args, cursor, spilled| {
        let params: P = lift_parameters(cx, ty, args, cursor, spilled)?;
        Ok(Box::new(params) as Box<dyn Any + Send>)
    }))
}

/// The parameter tuple a typed registration's lift produced.
fn typed_args<P: 'static>(args: HostArgs) -> Result<P> {
    match args {
        HostArgs::Typed(params) => params
            .downcast::<P>()
            .map(|params| *params)
            .map_err(|_| Error::internal("a typed registration was handed another tuple")),
        HostArgs::Vals(_) => Err(Error::internal(
            "a typed registration was handed untyped arguments",
        )),
    }
}

/// The arguments an untyped registration takes.
fn untyped_args(args: HostArgs) -> Result<Vec<Val>> {
    match args {
        HostArgs::Vals(args) => Ok(args),
        HostArgs::Typed(_) => Err(Error::internal(
            "an untyped registration was handed typed arguments",
        )),
    }
}

/// The failure a concurrent registration's future reports when the
/// vector it completed with is not the length the registration's
/// declared result asks for: one value when there is a result and
/// none when there is not.
///
/// The cause is the one an untyped synchronous registration's
/// mismatched value carries, because it is the same mistake seen a
/// call later — the host answered with something the declared type
/// does not describe — and `declared` names the result at issue, or
/// nothing when the declaration has no result to name.
fn result_arity(declared: Option<ValueType>) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Result,
        valtype: declared,
        cause: AbiCause::HostValueMismatch,
    })
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use crate::component::FunctionParameter;
    use crate::engine::Engine;
    use crate::store::Store;
    use crate::types::PrimitiveType;

    use super::super::host_func::ConcurrentHostFuncBody;
    use super::*;

    /// A future that is pending the first time it is polled and ready
    /// afterwards: one await for a registered closure's `async` block
    /// to hold the accessor across.
    ///
    /// It wakes the waker it was polled with before it parks. The
    /// tests await it inside the `run_concurrent` entry, and that
    /// entry parks when the store goes idle with the closure
    /// unfinished, so a future with nothing outside the store to wake
    /// it has to wake the turn itself — which is what a future waiting
    /// on the host's executor, a timer or a promise, does for it.
    #[derive(Default)]
    struct PendingOnce(bool);

    impl Future for PendingOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// The declared type of a function that takes one `u32` and
    /// returns one `u32`, carrying the `async` effect: the type an
    /// async-typed import of that shape has, which is what a host
    /// declares at an untyped concurrent registration.
    fn async_typed() -> FunctionType {
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "arg0".to_owned(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::U32)),
            async_: true,
        }
    }

    /// The body of a concurrent registration, or a failure naming what
    /// the registration turned out to be.
    fn concurrent<T: 'static>(host: &HostFunc<T>) -> &ConcurrentHostFuncBody<T> {
        match &host.kind {
            HostFuncKind::Concurrent(start, _) => start.as_ref(),
            HostFuncKind::Synchronous(..) => panic!("the registration is a synchronous one"),
        }
    }

    #[wcmp_macros::test]
    fn it_records_a_concurrent_kind_and_the_derived_type_for_the_typed_entry() {
        let mut registration = InstanceRegistration::<String>::new();

        LinkerInstance::new(&mut registration, false)
            .func_wrap_concurrent("greet", |accessor: &Accessor<String>, (extra,): (u32,)| {
                let accessor = accessor.clone();
                async move {
                    let reached = accessor.with(|access| access.data().len())?;
                    Ok((u32::try_from(reached).expect("the host data's length") + extra,))
                }
            })
            .expect("the registration");

        let host = registration.func("greet").expect("the registration");
        assert!(
            matches!(host.signature, HostSignature::Typed(_)),
            "the resolver checks the closure's types, as it checks `func_wrap`'s"
        );
        assert!(
            matches!(host.kind, HostFuncKind::Concurrent(_, HostParams::Typed(_))),
            "the registration records the concurrent kind and lifts its parameters itself"
        );
    }

    #[wcmp_macros::test]
    fn it_records_a_concurrent_kind_and_the_declared_type_for_the_untyped_entry() {
        let mut registration = InstanceRegistration::<String>::new();

        LinkerInstance::new(&mut registration, false)
            .func_new_concurrent(
                "greet",
                async_typed(),
                |_accessor: &Accessor<String>, args: Vec<Val>| async move { Ok(args) },
            )
            .expect("the registration");

        let host = registration.func("greet").expect("the registration");
        assert!(
            matches!(&host.signature, HostSignature::Declared(ty) if *ty == async_typed()),
            "the type declared at the registration is the one the resolver sees, whole"
        );
        assert!(
            matches!(host.kind, HostFuncKind::Concurrent(_, HostParams::Vals)),
            "the registration records the concurrent kind"
        );
    }

    #[wcmp_macros::test]
    fn it_records_a_synchronous_kind_for_the_two_synchronous_entries() {
        let mut registration = InstanceRegistration::<String>::new();
        let mut instance = LinkerInstance::new(&mut registration, false);

        instance
            .func_new("untyped", async_typed(), |_call, _args, _results| Ok(()))
            .expect("the registration");
        instance
            .func_wrap("typed", |_call: HostCall<'_, String>, (arg,): (u32,)| {
                Ok((arg,))
            })
            .expect("the registration");

        for name in ["untyped", "typed"] {
            let host = registration.func(name).expect("the registration");
            assert!(
                matches!(host.kind, HostFuncKind::Synchronous(..)),
                "the synchronous entries record the synchronous kind, for `{name}`"
            );
        }
    }

    #[wcmp_macros::test]
    async fn it_awaits_and_then_reaches_the_host_data_through_the_accessor() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");
        let mut registration = InstanceRegistration::<String>::new();

        LinkerInstance::new(&mut registration, false)
            .func_wrap_concurrent("greet", |accessor: &Accessor<String>, (extra,): (u32,)| {
                // The accessor is a token, so the block owns a clone of
                // it and holds that clone across the await. What it
                // reaches, it reaches in a later poll of the future.
                let accessor = accessor.clone();
                async move {
                    PendingOnce::default().await;
                    accessor.with(|mut access| {
                        let reached = access.data().len() as u32 + extra;
                        access.data_mut().push_str(&format!(" reached {reached}"));
                    })?;
                    Ok(())
                }
            })
            .expect("the registration");

        let host = registration
            .func("greet")
            .expect("the registration")
            .clone();
        let results = store
            .run_concurrent(async |accessor: &Accessor<String>| {
                concurrent(&host)(accessor, HostArgs::Typed(Box::new((2_u32,)))).await
            })
            .await
            .expect("run the closure")
            .expect("the call's results");

        assert!(
            matches!(results, HostResults::Typed(_)),
            "the closure's return crossed as its own tuple"
        );
        assert_eq!(
            store.data(),
            "host data reached 11",
            "what the future wrote to the host data, after its await, stayed there: the \
             length of \"host data\" and the argument"
        );
    }

    #[wcmp_macros::test]
    async fn it_fails_a_call_whose_result_vector_is_not_the_declared_length() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, ()).expect("store");
        let mut registration = InstanceRegistration::<()>::new();

        // The declared type names one result, and the future completes
        // with none.
        LinkerInstance::new(&mut registration, false)
            .func_new_concurrent(
                "greet",
                async_typed(),
                |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(Vec::new()) },
            )
            .expect("the registration");

        let host = registration
            .func("greet")
            .expect("the registration")
            .clone();
        let failure = store
            .run_concurrent(async |accessor: &Accessor<()>| {
                concurrent(&host)(accessor, HostArgs::Vals(vec![Val::U32(2)])).await
            })
            .await
            .expect("run the closure")
            .err()
            .expect("the call fails");

        assert_eq!(
            failure.to_string(),
            Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: Some(ValueType::Primitive(PrimitiveType::U32)),
                cause: AbiCause::HostValueMismatch,
            })
            .to_string(),
            "a result vector of the wrong length fails the call, naming the declared result"
        );
    }

    /// The browser bound takes a future that is not `Send`. A closure
    /// whose `async` block awaits a JavaScript promise and then reaches
    /// the host data is the browser host function the bound exists for,
    /// and the typed entry takes it.
    #[cfg(target_arch = "wasm32")]
    #[wcmp_macros::test]
    async fn it_takes_a_closure_whose_block_awaits_a_promise_in_the_browser() {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let mut store = Store::new(&engine, "host data".to_owned()).expect("store");
        let mut registration = InstanceRegistration::<String>::new();

        LinkerInstance::new(&mut registration, false)
            .func_wrap_concurrent("greet", |accessor: &Accessor<String>, (extra,): (u32,)| {
                let accessor = accessor.clone();
                async move {
                    let promise = js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_f64(3.0));
                    let resolved = wasm_bindgen_futures::JsFuture::from(promise)
                        .await
                        .expect("the promise resolves");
                    let from_promise = resolved.as_f64().expect("the promise's value") as u32;
                    accessor.with(|mut access| {
                        let reached = access.data().len() as u32 + from_promise + extra;
                        access.data_mut().push_str(&format!(" reached {reached}"));
                    })?;
                    Ok(())
                }
            })
            .expect("the registration");

        let host = registration
            .func("greet")
            .expect("the registration")
            .clone();
        let results = store
            .run_concurrent(async |accessor: &Accessor<String>| {
                concurrent(&host)(accessor, HostArgs::Typed(Box::new((2_u32,)))).await
            })
            .await
            .expect("run the closure")
            .expect("the call's results");

        assert!(matches!(results, HostResults::Typed(_)));
        assert_eq!(
            store.data(),
            "host data reached 14",
            "the future awaited the promise and then reached the host data: the \
             length of \"host data\", the promise's value, and the argument"
        );
    }
}
