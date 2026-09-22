//! A view onto a single instance-typed component export.
//!
//! Returned from [`InstanceExports::instance`] and from
//! [`ExportInstance::instance`]. An `ExportInstance` carries the path
//! of names it was addressed by, from the root of the component's
//! exports down to itself, and borrows from the parent [`Instance`]
//! for the duration of its lifetime. Its accessors look up a function
//! export or a further instance export inside the addressed instance.
//!
//! [`Instance`]: super::Instance
//! [`InstanceExports::instance`]: super::InstanceExports::instance

use crate::component::ExternalName;
use crate::internal::{ExportInstanceInternal, InstanceInternal};
use crate::module::Module;

use super::export_lookup::ExportLookup;
use super::func::Func;
use super::instance::Instance;

/// A view onto a single instance-typed export of an [`Instance`].
///
/// Returned by [`InstanceExports::instance`] for a root-level
/// instance export and by [`Self::instance`] for an instance export
/// nested inside another. The view scopes [`Self::func`] and
/// [`Self::instance`] lookups to the items declared inside the
/// addressed instance.
///
/// [`Instance`]: super::Instance
/// [`InstanceExports::instance`]: super::InstanceExports::instance
pub struct ExportInstance<'a> {
    instance: &'a Instance,
    /// The names from the root of the export tree down to this
    /// instance, the last of which is this instance's own name.
    /// Never empty: a view is only built for an export that exists.
    path: Box<[ExternalName]>,
}

impl<'a> ExportInstanceInternal<'a> for ExportInstance<'a> {
    fn new(instance: &'a Instance, path: Box<[ExternalName]>) -> Self {
        ExportInstance { instance, path }
    }
}

impl<'a> ExportInstance<'a> {
    /// The name this instance-typed export is declared under: a WIT
    /// interface identifier or a plain name, as the component
    /// published it.
    pub fn name(&self) -> &ExternalName {
        self.path
            .last()
            .expect("an export instance view holds at least its own name")
    }

    /// Look up a function export inside the addressed instance by
    /// its declared item name. Returns `None` if no such function
    /// is present.
    pub fn func(&self, name: &str) -> Option<Func> {
        self.instance.function_export(&self.path, name)
    }

    /// Look up a core module export inside the addressed instance by
    /// its declared item name. Returns `None` if no such module is
    /// present.
    pub fn module(&self, name: &str) -> Option<Module> {
        self.instance.module_export(&self.path, name)
    }

    /// Look up an instance-typed export nested inside the addressed
    /// instance. The name is anything that implements
    /// [`ExportLookup`], as for [`InstanceExports::instance`].
    /// Returns `None` if no nested instance export carries the name.
    ///
    /// [`InstanceExports::instance`]: super::InstanceExports::instance
    pub fn instance(&self, name: impl ExportLookup) -> Option<ExportInstance<'a>> {
        let mut path = self.path.to_vec();
        path.push(name.external_name());
        let path = path.into_boxed_slice();
        if self.instance.has_instance_export(&path) {
            Some(ExportInstance::new(self.instance, path))
        } else {
            None
        }
    }
}
