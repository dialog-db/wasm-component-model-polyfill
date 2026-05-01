//! The polyfill's export navigator.
//!
//! [`InstanceExports`] is the value [`Instance::exports`] returns. It
//! borrows from the instance and exposes two lookups: one keyed by
//! function-name that returns the polyfill's [`Func`] for a root-
//! level function export, and one keyed by [`InterfaceIdentifier`]
//! that returns an [`ExportInstance`] view onto a single instance-
//! typed export.
//!
//! The navigator is the polyfill's own type — no upstream type
//! appears at the navigation boundary. The instance lookup uses the
//! polyfill's own [`InterfaceIdentifier`] so traversal threads
//! through the identifier surface introduced earlier in the project.
//!
//! [`Instance`]: super::Instance
//! [`Instance::exports`]: super::Instance::exports

use crate::identifier::InterfaceIdentifier;

use super::export_instance::ExportInstance;
use super::func::Func;
use super::instance::Instance;

/// The export navigator for a successfully linked, instantiated
/// [`Instance`].
///
/// `InstanceExports` is obtained from [`Instance::exports`] and
/// borrows from the instance for its lifetime. Two lookups are
/// reachable here: [`Self::func`] for root-level function exports
/// and [`Self::instance`] for instance-typed exports addressed by
/// the polyfill's [`InterfaceIdentifier`]. Nested function exports
/// are reachable only through the latter.
///
/// The flat-name shorthand [`Instance::get_func`] is preserved
/// unchanged; it is the same root-level lookup [`Self::func`]
/// performs.
///
/// [`Instance`]: super::Instance
/// [`Instance::exports`]: super::Instance::exports
/// [`Instance::get_func`]: super::Instance::get_func
pub struct InstanceExports<'a> {
    instance: &'a Instance,
}

impl<'a> InstanceExports<'a> {
    /// Construct the navigator for an [`Instance`].
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    ///
    /// [`Instance`]: super::Instance
    pub fn new(instance: &'a Instance) -> Self {
        Self { instance }
    }

    /// Look up a root-level exported function by its declared name.
    /// Returns `None` if no root-level function export carries the
    /// given name; nested function exports inside an instance-typed
    /// export are reachable only through [`Self::instance`].
    pub fn func(&self, name: &str) -> Option<Func> {
        self.instance
            .function_exports
            .iter()
            .find(|export| export.parent.is_none() && export.name == name)
            .map(|export| Func {
                name: export.name.clone(),
                inner: export.func.clone(),
                signature: export.signature.clone(),
                options: export.options.clone(),
                abi_state: self.instance.abi_state.clone(),
            })
    }

    /// Look up an instance-typed export addressed by its
    /// [`InterfaceIdentifier`]. Returns `None` if the instance is
    /// absent. The returned [`ExportInstance`] reaches the
    /// instance's nested function exports through its own
    /// [`ExportInstance::func`] accessor.
    pub fn instance(&self, identifier: &InterfaceIdentifier) -> Option<ExportInstance<'a>> {
        let any_match = self
            .instance
            .function_exports
            .iter()
            .any(|export| export.parent.as_ref() == Some(identifier));
        if any_match {
            Some(ExportInstance::new(self.instance, identifier.clone()))
        } else {
            None
        }
    }
}
