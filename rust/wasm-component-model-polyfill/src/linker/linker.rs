//! The polyfill's host-environment build-up.

use core::marker::PhantomData;
use std::collections::HashMap;

use crate::component::Component;
use crate::engine::Engine;
use crate::error::Result;
use crate::identifier::InterfaceIdentifier;
use crate::instance::Instance;
use crate::store::Store;

use super::linker_instance::LinkerInstance;
use super::registration::InstanceRegistration;
use super::resolve::{Resolution, resolve_imports};

/// The polyfill's host-environment build-up.
///
/// `Linker<T>` is constructed from an [`Engine`] via [`Linker::new`]
/// and is the value the polyfill instantiates a [`Component`]
/// through. `T` matches the host-data parameter of the [`Store<T>`]
/// the linker is later used with.
///
/// The linker organises host items by interface: a
/// [`LinkerInstance`] borrowed from a `Linker` is the unit of "a
/// single interface's worth of host items," addressed by a
/// [`PackageName`] and an [`InterfaceIdentifier`].
///
/// Plain-named imports resolve through the root namespace,
/// addressed with [`Linker::root`]. A component whose imports have
/// no matching registration fails cleanly with [`Error::Link`].
///
/// [`Component`]: crate::Component
/// [`PackageName`]: crate::PackageName
/// [`Error::Link`]: crate::Error::Link
pub struct Linker<T> {
    engine: Engine,
    instances: HashMap<InterfaceIdentifier, InstanceRegistration<T>>,
    /// The root namespace: host items a component imports under a
    /// plain name rather than an interface identifier.
    root: InstanceRegistration<T>,
    _phantom: PhantomData<fn(T) -> T>,
}

impl<T: 'static> Linker<T> {
    /// Construct an empty `Linker` against an [`Engine`].
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            instances: HashMap::new(),
            root: InstanceRegistration::new(),
            _phantom: PhantomData,
        }
    }

    /// The [`Engine`] this linker was constructed against.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The set of registered linker-instance keys, in insertion order
    /// from the underlying map's iteration order. Used by the
    /// resolver to enumerate candidates when matching a component's
    /// imports.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn registered_keys(&self) -> impl Iterator<Item = &InterfaceIdentifier> {
        self.instances.keys()
    }

    /// The registration entry for a given interface identifier, or
    /// `None` when no matching entry exists. Used by the resolver
    /// to look up host-function payloads by interface and by the
    /// host-trampoline builder to dispatch a lowered import.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn registration_for(&self, id: &InterfaceIdentifier) -> Option<&InstanceRegistration<T>> {
        self.instances.get(id)
    }

    /// The root namespace's registration entry. Consulted by the
    /// resolver for plain-named imports.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn root_registration(&self) -> &InstanceRegistration<T> {
        &self.root
    }

    /// Address the root namespace: the host items a component
    /// imports under a plain name, for example
    /// `(import "log" (func …))`. The view is the same
    /// [`LinkerInstance`] an interface accessor returns, so the
    /// registration operations are the same. A plain-named instance
    /// import, `(import "host" (instance …))`, is addressed through
    /// [`LinkerInstance::instance`] on this view.
    ///
    /// Calling `root` twice returns a view onto the same entry.
    pub fn root(&mut self) -> LinkerInstance<'_, T> {
        LinkerInstance::new(&mut self.root)
    }

    /// Address (creating if absent) the [`LinkerInstance`] keyed by
    /// the given interface identifier.
    ///
    /// Calling `instance` with the same identifier twice returns a
    /// view onto the same registration entry. The semver value
    /// captured in the identifier participates in resolution per the
    /// WIT compatibility rules; see the polyfill's identifier
    /// resolution module for the matching semantics.
    pub fn instance(&mut self, id: &InterfaceIdentifier) -> LinkerInstance<'_, T> {
        let entry = self.instances.entry(id.clone()).or_default();
        LinkerInstance::new(entry)
    }

    /// Instantiate a [`Component`] into the given [`Store`].
    ///
    /// Resolves every declared import of the component against the
    /// linker's registered linker instances using WIT-spec semver
    /// compatibility, then drives the underlying runtime substrate
    /// through the resulting wiring. The returned [`Instance`]'s
    /// lifetime is bound to `store`; multiple instances of the same
    /// component can coexist in a single store and remain isolated.
    ///
    /// Failures fall into three groups:
    ///
    /// - Resolution failures (missing match, ambiguous match,
    ///   unsatisfiable semver, registration of an item this PDD
    ///   does not yet support) surface as [`Error::Link`].
    /// - Substrate-level instantiation failures surface as
    ///   [`Error::Instantiation`] with the runtime cause captured
    ///   as `#[source]`.
    /// - Component shapes the polyfill intentionally rejects
    ///   (e.g. exports whose signatures require compound-valtype
    ///   lift/lower the polyfill defers) surface as
    ///   [`Error::Instantiation`] with a structured reason and no
    ///   underlying cause.
    ///
    /// [`Error::Link`]: crate::Error::Link
    /// [`Error::Instantiation`]: crate::Error::Instantiation
    pub fn instantiate(&self, store: &mut Store<T>, component: &Component) -> Result<Instance> {
        let resolution = resolve_imports(component, self)?;
        self.instantiate_resolved(store, component, &resolution)
    }

    /// Drive the runtime substrate against an already-resolved
    /// import binding. Split out from [`Self::instantiate`] so the
    /// native and web paths plug in only the substrate-specific
    /// step; identifier resolution is shared.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn instantiate_resolved(
        &self,
        store: &mut Store<T>,
        component: &Component,
        resolution: &Resolution,
    ) -> Result<Instance> {
        crate::executor::instantiate(component, store, self, resolution)
    }
}
