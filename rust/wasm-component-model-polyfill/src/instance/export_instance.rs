//! A view onto a single instance-typed component export.
//!
//! Returned from [`InstanceExports::instance`]. An `ExportInstance`
//! carries the [`InterfaceIdentifier`] it was addressed by and
//! borrows from the parent [`Instance`] for the duration of its
//! lifetime. Its single accessor — [`Self::func`] — looks up a
//! function export inside the addressed instance.
//!
//! [`Instance`]: super::Instance
//! [`InstanceExports::instance`]: super::InstanceExports::instance

use crate::identifier::InterfaceIdentifier;

use super::func::Func;
use super::instance::Instance;

/// A view onto a single instance-typed export of an [`Instance`].
///
/// Returned by [`InstanceExports::instance`]. The view scopes
/// [`Self::func`] lookups to the items declared inside the addressed
/// instance.
///
/// [`Instance`]: super::Instance
/// [`InstanceExports::instance`]: super::InstanceExports::instance
pub struct ExportInstance<'a> {
    instance: &'a Instance,
    identifier: InterfaceIdentifier,
}

impl<'a> ExportInstance<'a> {
    /// Construct a view onto an instance-typed export by its
    /// [`InterfaceIdentifier`].
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(instance: &'a Instance, identifier: InterfaceIdentifier) -> Self {
        Self {
            instance,
            identifier,
        }
    }

    /// The [`InterfaceIdentifier`] this view is addressed by.
    pub fn identifier(&self) -> &InterfaceIdentifier {
        &self.identifier
    }

    /// Look up a function export inside the addressed instance by
    /// its declared item name. Returns `None` if no such function
    /// is present.
    pub fn func(&self, name: &str) -> Option<Func> {
        self.instance
            .function_exports
            .iter()
            .find(|export| export.parent.as_ref() == Some(&self.identifier) && export.name == name)
            .map(|export| Func {
                name: export.name.clone(),
                inner: export.func.clone(),
                signature: export.signature.clone(),
                options: export.options.clone(),
                abi_state: self.instance.abi_state.clone(),
            })
    }
}
