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

use super::host_func::HostFunc;

/// The polyfill's owned, per-interface registration entry.
///
/// `T` is the [`Store`]'s host-data type. Each interface carries
/// an ordered map from item-name to [`HostFunc<T>`]; the order is
/// insertion order so that diagnostics and iteration are
/// deterministic.
///
/// [`Store`]: crate::Store
pub struct InstanceRegistration<T> {
    /// The host-function payloads keyed by the item-name the
    /// component-side import expects. Insertion order is preserved
    /// via the `BTreeMap`'s sorted iteration; the keys are
    /// item-names which are short and inexpensive to compare.
    pub funcs: BTreeMap<String, HostFunc<T>>,
}

impl<T> InstanceRegistration<T> {
    /// Construct an empty registration.
    pub fn new() -> Self {
        Self {
            funcs: BTreeMap::new(),
        }
    }

    /// Look up a registered host function by its item-name.
    pub fn func(&self, name: &str) -> Option<&HostFunc<T>> {
        self.funcs.get(name)
    }
}

impl<T> Default for InstanceRegistration<T> {
    fn default() -> Self {
        Self::new()
    }
}
