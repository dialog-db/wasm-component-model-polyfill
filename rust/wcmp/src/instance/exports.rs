// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill's export navigator.
//!
//! [`InstanceExports`] is the value [`Instance::exports`] returns. It
//! borrows from the instance and exposes two lookups: one keyed by
//! function name that returns the polyfill's [`Func`] for a root-
//! level function export, and one keyed by an [`ExportLookup`] name
//! that returns an [`ExportInstance`] view onto a single instance-
//! typed export, whether the component published it under a WIT
//! interface identifier or under a plain name.
//!
//! The navigator is the polyfill's own type — no upstream type
//! appears at the navigation boundary. An instance lookup accepts
//! the polyfill's own [`InterfaceIdentifier`] as well as a string, so
//! traversal threads through the identifier surface introduced
//! earlier in the project without forcing a parse on a plain name.
//!
//! [`Instance`]: super::Instance
//! [`Instance::exports`]: super::Instance::exports
//! [`InterfaceIdentifier`]: crate::InterfaceIdentifier

use crate::internal::{ExportInstanceInternal, InstanceExportsInternal, InstanceInternal};
use crate::module::Module;

use super::export_instance::ExportInstance;
use super::export_lookup::ExportLookup;
use super::func::Func;
use super::instance::Instance;

/// The export navigator for a successfully linked, instantiated
/// [`Instance`].
///
/// `InstanceExports` is obtained from [`Instance::exports`] and
/// borrows from the instance for its lifetime. Three lookups are
/// reachable here: [`Self::func`] for root-level function exports,
/// [`Self::module`] for root-level core module exports, and
/// [`Self::instance`] for instance-typed exports addressed by name.
/// Nested function and module exports are reachable only through the
/// latter, and an instance nested inside an instance is reached
/// through [`ExportInstance::instance`] on the outer view.
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

impl<'a> InstanceExportsInternal<'a> for InstanceExports<'a> {
    fn new(instance: &'a Instance) -> Self {
        InstanceExports { instance }
    }
}

impl<'a> InstanceExports<'a> {
    /// Look up a root-level exported function by its declared name.
    /// Returns `None` if no root-level function export carries the
    /// given name; nested function exports inside an instance-typed
    /// export are reachable only through [`Self::instance`].
    pub fn func(&self, name: &str) -> Option<Func> {
        self.instance.function_export(&[], name)
    }

    /// Look up a root-level exported core module by its declared
    /// name. Returns `None` if no root-level module export carries
    /// the given name.
    pub fn module(&self, name: &str) -> Option<Module> {
        self.instance.module_export(&[], name)
    }

    /// Look up a root-level instance-typed export by name. The name
    /// is anything that implements [`ExportLookup`]: a plain string
    /// such as `"a"`, a string in WIT interface-name syntax such as
    /// `"test:guest/foo"`, or a parsed [`InterfaceIdentifier`].
    /// Returns `None` if no instance-typed export carries the name.
    /// The returned [`ExportInstance`] reaches the instance's nested
    /// function exports through [`ExportInstance::func`] and its
    /// nested instance exports through [`ExportInstance::instance`].
    ///
    /// [`InterfaceIdentifier`]: crate::InterfaceIdentifier
    pub fn instance(&self, name: impl ExportLookup) -> Option<ExportInstance<'a>> {
        let path = vec![name.external_name()].into_boxed_slice();
        if self.instance.has_instance_export(&path) {
            Some(ExportInstance::new(self.instance, path))
        } else {
            None
        }
    }
}
