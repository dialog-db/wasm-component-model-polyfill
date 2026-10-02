// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Core modules at the component boundary.
//!
//! A component can export a core module for the host to inspect and
//! instantiate itself, and a host can load a core module from bytes.
//! [`Module`] is the polyfill's handle for either: a compiled core
//! module with the types of its imports and exports, that the host
//! instantiates into a [`CoreInstance`] with a list of [`CoreExtern`]
//! values. Every type here is polyfill-owned, so no runtime-layer type
//! reaches the public API.
//!
//! [`CoreInstance`]: crate::CoreInstance
//! [`CoreExtern`]: crate::CoreExtern

mod core_extern;
mod core_extern_type;
mod core_instance;
mod core_value_type;
#[allow(clippy::module_inception)]
mod module;
mod module_export;
mod module_import;
mod read;

pub use core_extern::CoreExtern;
pub use core_extern_type::CoreExternType;
pub use core_instance::CoreInstance;
pub use core_value_type::CoreValueType;
pub use module::Module;
pub use module_export::ModuleExport;
pub use module_import::ModuleImport;
