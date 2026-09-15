//! The owned, per-interface state a [`Linker`] holds for each
//! registered [`LinkerInstance`].
//!
//! Each registration carries an ordered map from item-name to
//! [`HostFunc`]: the host-function payloads attached to one
//! interface's worth of items. The order is insertion order; the
//! resolver matches by name when checking a host registration
//! against a component's declared imports.
//!
//! [`Linker`]: super::Linker
//! [`LinkerInstance`]: super::LinkerInstance

use std::collections::BTreeMap;

use crate::module::Module;

use super::host_func::HostFunc;
use super::host_resource::HostResource;

/// The polyfill's owned, per-interface registration entry.
///
/// `T` is the [`Store`]'s host-data type. Each interface carries
/// an ordered map from item-name to [`HostFunc<T>`] alongside an
/// ordered map for [`HostResource<T>`] registrations; both orders
/// are insertion order so that diagnostics and iteration are
/// deterministic.
///
/// [`Store`]: crate::Store
pub struct InstanceRegistration<T> {
    /// The host-function payloads keyed by the item-name the
    /// component-side import expects. Insertion order is preserved
    /// via the `BTreeMap`'s sorted iteration; the keys are
    /// item-names which are short and inexpensive to compare.
    pub funcs: BTreeMap<String, HostFunc<T>>,
    /// The host-resource registrations keyed by the resource-type
    /// label the component-side import declares.
    pub resources: BTreeMap<String, HostResource<T>>,
    /// The core modules registered for module-typed imports, keyed
    /// by the name the component-side import declares.
    pub modules: BTreeMap<String, Module>,
    /// Nested registrations for plain-named instance imports, keyed
    /// by the plain name. Only the root registration holds these: a
    /// component that imports `(instance)` under a plain name finds
    /// its items here.
    pub instances: BTreeMap<String, InstanceRegistration<T>>,
}

impl<T> InstanceRegistration<T> {
    /// Construct an empty registration.
    pub fn new() -> Self {
        Self {
            funcs: BTreeMap::new(),
            resources: BTreeMap::new(),
            modules: BTreeMap::new(),
            instances: BTreeMap::new(),
        }
    }

    /// Look up the nested registration for a plain-named instance.
    pub fn instance(&self, name: &str) -> Option<&InstanceRegistration<T>> {
        self.instances.get(name)
    }

    /// Look up a registered host function by its item-name.
    pub fn func(&self, name: &str) -> Option<&HostFunc<T>> {
        self.funcs.get(name)
    }

    /// Look up a registered host resource by its label.
    pub fn resource(&self, label: &str) -> Option<&HostResource<T>> {
        self.resources.get(label)
    }

    /// Look up a registered core module by its name.
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.get(name)
    }
}

impl<T> Default for InstanceRegistration<T> {
    fn default() -> Self {
        Self::new()
    }
}
