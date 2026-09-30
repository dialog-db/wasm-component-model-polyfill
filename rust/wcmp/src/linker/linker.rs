//! The polyfill's host-environment build-up.

use core::future::poll_fn;
use core::marker::PhantomData;
use core::task::Poll;
use std::collections::HashMap;

use crate::component::Component;
use crate::concurrency::TurnGuard;
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
/// A name, once registered, is taken. A second registration under
/// one name inside one [`LinkerInstance`] — a second function,
/// resource, or module, whichever kind took the name first — fails
/// at the registration call with [`Error::Link`] rather than
/// replacing what is there, so a host that registers the same item
/// twice by accident hears about it where the mistake is rather
/// than losing one of the two silently. Addressing an instance,
/// with [`Linker::root`], [`Linker::instance`], or
/// [`LinkerInstance::instance`], is not a registration: it answers
/// with the entry already there.
///
/// [`Linker::allow_shadowing`] is the escape hatch. A linker told
/// to allow shadowing lets every later registration replace what
/// stands under its name, which is what a host that layers its own
/// items over a set it did not assemble — a WASI set, say — wants.
/// The setting reaches the views the linker hands out after it is
/// set, and the default is off, as it is in Wasmtime.
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
    /// Whether a registration may take a name that is already taken.
    /// Off by default; every view the linker hands out carries it.
    allow_shadowing: bool,
    _phantom: PhantomData<fn(T) -> T>,
}

impl<T: 'static> Linker<T> {
    /// Construct an empty `Linker` against an [`Engine`].
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            instances: HashMap::new(),
            root: InstanceRegistration::new(),
            allow_shadowing: false,
            _phantom: PhantomData,
        }
    }

    /// Configure whether a registration may take a name that is
    /// already taken.
    ///
    /// Off by default: a second registration under one name inside
    /// one [`LinkerInstance`] fails with [`Error::Link`]. Turned on,
    /// a registration replaces whatever stands under its name, and
    /// the last one made is the one the resolver sees.
    ///
    /// The setting is read when the linker hands out a view, so it
    /// governs the registrations made after it is set, on the views
    /// taken after it is set.
    ///
    /// [`Error::Link`]: crate::Error::Link
    pub fn allow_shadowing(&mut self, allow: bool) -> &mut Self {
        self.allow_shadowing = allow;
        self
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
        let allow_shadowing = self.allow_shadowing;
        LinkerInstance::new(&mut self.root, allow_shadowing)
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
        let allow_shadowing = self.allow_shadowing;
        let entry = self.instances.entry(id.clone()).or_default();
        LinkerInstance::new(entry, allow_shadowing)
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
    /// An instantiation that fails leaves the store's own records as
    /// it found them: the instance records the plan reserved, the
    /// destructors its resource types registered, and the names it
    /// taught for them all go back. Wasmtime keeps what a failed
    /// instantiation left in its store, so this is hygiene rather
    /// than parity — no instance of either store can reach the
    /// records of an instantiation that produced none. The work the
    /// initializers left behind is not part of it, as above: that is
    /// guest work the store has taken on, and it stays.
    ///
    /// Entering instantiation while another driver of the same store
    /// is inside a turn fails with the recursive-driver cause, and a
    /// turn that goes idle with the plan unfinished fails with the
    /// deadlock cause.
    ///
    /// A core `start` function that traps poisons the store. A store
    /// a trap poisoned refuses the instantiation with the
    /// cannot-enter cause, [`TaskCause::CannotEnter`], once the
    /// imports have resolved. Wasmtime lets such an instantiation
    /// run; the polyfill refuses it, because it runs `start`
    /// functions, and no guest code runs in a store after a trap. A
    /// link error does not poison the store, because it is raised
    /// before any guest code runs.
    ///
    /// [`TaskCause::CannotEnter`]: crate::TaskCause::CannotEnter
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
        // An instantiation runs the `start` functions of its core
        // modules, which is guest code, so a store a trap poisoned
        // refuses it.
        store.internal().enter_guest()?;
        // The plan runs inside a turn, as all guest work does, and the
        // turn lasts until the plan is done: a core instantiation is
        // asynchronous, and the turn waits for it with nothing else in
        // between.
        let waker = poll_fn(|context| Poll::Ready(context.waker().clone())).await;
        let tables = store.internal().tables_handle();
        let _turn = TurnGuard::enter(&tables, &waker);
        self.instantiate_resolved(&mut store, component, &resolution)
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

    async fn instantiate_resolved(
        &self,
        store: &mut StoreContext<'_, T>,
        component: &Component,
        resolution: &Resolution,
    ) -> Result<Instance> {
        crate::executor::instantiate(component, store, self, resolution).await
    }
}
