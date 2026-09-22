//! The polyfill's host-environment build-up.

use core::marker::PhantomData;
use std::collections::HashMap;

use crate::component::Component;
use crate::concurrency::Driver;
use crate::engine::Engine;
use crate::error::{Error, Result, SchedulerCause};
use crate::identifier::InterfaceIdentifier;
use crate::instance::Instance;
use crate::internal::{LinkerInstanceInternal, LinkerInternal};
use crate::store::{Store, StoreContext};
use crate::store::{StoreContextInternalExt, StoreInternalExt};

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
/// Every import that is not an interface-named instance resolves
/// through the root namespace, addressed with [`Linker::root`],
/// under the import's own name written out in full. That is the
/// plain-named imports, and it is also a function, resource, or
/// module import written under an interface name, which asks for a
/// single host item rather than an interface's worth of them.
///
/// The root matches a versioned name the way a registered interface
/// key is matched, because a root name is a name like any other: a
/// registration under `pkg:ns/iface@0.1.0` answers an import of
/// `pkg:ns/iface@0.1.3`, while a registration under the import's
/// exact name answers ahead of any merely compatible one. A plain
/// name carries no version and so is only ever matched exactly. The
/// polyfill's identifier-resolution module states the version rules
/// in full.
///
/// A component whose imports have no matching registration fails
/// cleanly with [`Error::Link`].
///
/// [`Component`]: crate::Component
/// [`PackageName`]: crate::PackageName
/// [`Error::Link`]: crate::Error::Link
pub struct Linker<T: 'static> {
    // Held from construction so that a later entry can build
    // against the engine the linker was made for; nothing reads it
    // today.
    #[allow(dead_code)]
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

    /// Address the root namespace: the host items a component
    /// imports under a name of their own, for example
    /// `(import "log" (func …))`. The view is the same
    /// [`LinkerInstance`] an interface accessor returns, so the
    /// registration operations are the same. A plain-named instance
    /// import, `(import "host" (instance …))`, is addressed through
    /// [`LinkerInstance::instance`] on this view.
    ///
    /// An import that is not an instance resolves here whatever its
    /// name looks like, so a function import written under an
    /// interface name, `(import "pkg:ns/iface@0.1.0" (func …))`, is
    /// registered on this view under that whole name. Only an
    /// instance import named by an interface identifier goes to
    /// [`Linker::instance`] instead.
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
    /// resolver's version rules; see the polyfill's identifier
    /// resolution module for the matching semantics.
    pub fn instance(&mut self, id: &InterfaceIdentifier) -> LinkerInstance<'_, T> {
        let entry = self.instances.entry(id.clone()).or_default();
        LinkerInstance::new(entry)
    }

    /// Instantiate a [`Component`] into the given [`Store`].
    ///
    /// Resolves every declared import of the component against the
    /// linker's registered linker instances using the resolver's
    /// version rules, then drives the underlying runtime substrate
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
    ///
    /// Instantiation is a driver of the store's cooperative
    /// scheduler: the initializers of the plan and the core `start`
    /// functions they run are guest work, and guest work runs only
    /// inside a turn. The initializers are not queued items, because
    /// they run against plan state the caller lends for the duration
    /// of the call and an item outlives the driver that queued it;
    /// they run inside a turn the driver opens instead. Work the
    /// initializers leave behind stays in the store.
    ///
    /// Entering instantiation while another driver of the same store
    /// is inside a turn fails with the recursive-driver cause, and a
    /// turn that goes idle with the plan unfinished fails with the
    /// deadlock cause.
    pub async fn instantiate(
        &self,
        store: &mut Store<T>,
        component: &Component,
    ) -> Result<Instance> {
        let mut store = store.internal().context();
        let resolution = resolve_imports(component, self)?;
        if store.internal().turn_in_flight() {
            return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
        }
        let mut plan = Some(());
        Driver::new(
            store,
            None,
            move |store: &mut StoreContext<'_, T>, waker| {
                plan.take()?;
                Some(
                    store
                        .internal()
                        .run_in_turn(waker, |store| {
                            self.instantiate_resolved(store, component, &resolution)
                        })
                        .and_then(|outcome| outcome),
                )
            },
        )
        .await
    }
}

impl<T: 'static> LinkerInternal<T> for Linker<T> {
    fn registered_keys(&self) -> impl Iterator<Item = &InterfaceIdentifier> {
        self.instances.keys()
    }

    fn registration_for(&self, id: &InterfaceIdentifier) -> Option<&InstanceRegistration<T>> {
        self.instances.get(id)
    }

    fn root_registration(&self) -> &InstanceRegistration<T> {
        &self.root
    }

    fn instantiate_resolved(
        &self,
        store: &mut StoreContext<'_, T>,
        component: &Component,
        resolution: &Resolution,
    ) -> Result<Instance> {
        crate::executor::instantiate(component, store, self, resolution)
    }
}
