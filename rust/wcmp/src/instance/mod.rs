// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill's owner of a successfully linked, instantiated
//! component.
//!
//! [`Instance`] exposes an export-lookup accessor — given a name it
//! returns the polyfill's [`Func`] — and a calling convention that
//! drives the canonical-ABI round-trip of an exported component
//! function for every valtype in the synchronous baseline. Every
//! handle an instance hands out remembers the store the instance was
//! created in and refuses a call through any other store.

mod call_values;
mod export_instance;
mod export_lookup;
mod exports;
mod func;
#[allow(clippy::module_inception)]
mod instance;
mod typed_call;
mod typed_func;

pub use export_instance::ExportInstance;
pub use export_lookup::ExportLookup;
pub use exports::InstanceExports;
pub use func::Func;
pub use instance::{ExportedFunction, ExportedModule, Instance};
pub use typed_func::TypedFunc;
