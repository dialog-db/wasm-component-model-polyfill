//! A borrowed view onto one interface's registration in a [`Linker`].
//!
//! [`Linker`]: super::Linker

use core::marker::PhantomData;

use crate::component::FunctionType;
use crate::error::Result;
use crate::resource::ResourceTypeId;
use crate::value::Val;

use super::component_value::{ComponentParameters, ComponentResult, function_type_for};
use super::host_call::HostCall;
use super::host_func::HostFunc;
use super::host_resource::HostResource;
use super::registration::InstanceRegistration;

/// A borrowed view onto one interface's worth of host items inside a
/// [`Linker`].
///
/// `LinkerInstance` is the addressing surface for "the host items
/// that satisfy *this* interface": it is obtained from
/// [`Linker::instance`] and carries the back-reference the linker
/// uses to look up or insert a per-interface registration entry.
/// Two registration modes are exposed:
///
/// - [`Self::func_new`] (untyped): take an explicit
///   [`FunctionType`] and a closure over [`HostCall`],
///   `&[Val]`, `&mut [Val]`.
/// - [`Self::func_wrap`] (typed): take a closure with statically-
///   typed Rust arguments and return; the polyfill derives the
///   declared signature from the closure's generic parameters.
///
/// Both modes converge on a single [`HostFunc<T>`] that the linker
/// stores against the item's name, then the resolver's link-time
/// type check consults when matching the registration against the
/// component's declared import.
///
/// A third mode, [`Self::resource`], registers a host-owned
/// resource type with a synchronous destructor closure. It produces
/// a [`HostResource<T>`] that the linker stores against the
/// resource's label.
///
/// [`Linker`]: super::Linker
/// [`Linker::instance`]: super::Linker::instance
pub struct LinkerInstance<'a, T> {
    /// The owned registration this view borrows. Held mutably so
    /// later registration methods can populate it without further
    /// linker access. Workspace-internal: this field is not
    /// re-exported by `lib.rs` and never reaches downstream
    /// consumers.
    pub registration: &'a mut InstanceRegistration<T>,
    /// `T` participates only as the host-data type the registration
    /// carries; capture invariance explicitly so the parameter does
    /// not appear unused.
    _phantom: PhantomData<fn(T) -> T>,
}

impl<'a, T: 'static> LinkerInstance<'a, T> {
    /// Construct a borrowed view onto the given registration.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(registration: &'a mut InstanceRegistration<T>) -> Self {
        Self {
            registration,
            _phantom: PhantomData,
        }
    }

    /// Address (creating if absent) the nested registration for a
    /// plain-named instance import, `(import "name" (instance …))`.
    /// Meaningful on the root view a [`Linker::root`] returns: an
    /// interface-named import has no nested instances.
    ///
    /// [`Linker::root`]: super::Linker::root
    pub fn instance(&mut self, name: impl Into<String>) -> LinkerInstance<'_, T> {
        let entry = self.registration.instances.entry(name.into()).or_default();
        LinkerInstance::new(entry)
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
    pub fn func_new(
        &mut self,
        name: impl Into<String>,
        ty: FunctionType,
        func: impl for<'c> Fn(HostCall<'c, T>, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static,
    ) {
        let host = HostFunc::new(ty, func);
        self.registration.funcs.insert(name.into(), host);
    }

    /// Register a host-owned resource type with a synchronous
    /// destructor.
    ///
    /// Mints a fresh [`ResourceTypeId`] under the given resource
    /// label and stores the destructor closure for the executor's
    /// drop trampoline to invoke when the guest drops the last
    /// handle. The returned identity threads the registration into
    /// any handle the host subsequently mints.
    ///
    /// Today the polyfill registers exactly one resource per label
    /// per interface; calling `resource` twice with the same label
    /// overwrites the prior registration.
    pub fn resource(
        &mut self,
        label: impl Into<String>,
        destructor: impl Fn(&mut T, u32) -> Result<()> + Send + Sync + 'static,
    ) -> ResourceTypeId {
        self.resource_with(label, HostResource::new(destructor))
    }

    /// Register a host resource type by value. A [`HostResource`]
    /// carries its identity and its destructor, so one value, cloned,
    /// registers the same resource type under several interfaces: a
    /// handle minted under one lowers through the other, and the
    /// resolver accepts a component that declares the two as equal.
    /// Returns the identity for [`Store::resource_new`].
    ///
    /// [`Store::resource_new`]: crate::Store::resource_new
    pub fn resource_with(
        &mut self,
        label: impl Into<String>,
        resource: HostResource<T>,
    ) -> ResourceTypeId {
        let type_id = resource.type_id;
        self.registration.resources.insert(label.into(), resource);
        type_id
    }

    /// Register a *typed* host function. The closure's argument
    /// tuple and return type derive the declared [`FunctionType`]
    /// via [`ComponentParameters`] and [`ComponentResult`]; the
    /// linker's resolver checks the derived signature against the
    /// component's import at link time.
    pub fn func_wrap<Params, Ret, F>(&mut self, name: impl Into<String>, func: F)
    where
        Params: ComponentParameters,
        Ret: ComponentResult,
        F: for<'c> Fn(HostCall<'c, T>, Params) -> Result<Ret> + Send + Sync + 'static,
    {
        let signature = function_type_for::<Params, Ret>();
        let host = HostFunc::new(signature, move |call, args, results| {
            let params = Params::from_vals(args)?;
            let ret = func(call, params)?;
            if let Some(val) = ret.into_val()
                && !results.is_empty()
            {
                // The untyped path enforces that `results.len()`
                // matches the declared result count; an empty slot
                // here means the registered Rust signature
                // disagreed with the declared ABI, which the
                // link-time check should already have rejected.
                results[0] = val;
            }
            Ok(())
        });
        self.registration.funcs.insert(name.into(), host);
    }
}
