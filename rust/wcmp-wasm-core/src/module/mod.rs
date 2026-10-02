// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A compiled module, and the description of its boundary.

mod export_type;
mod import_type;
#[allow(clippy::module_inception)]
mod module;

pub use export_type::ExportType;
pub use import_type::ImportType;
pub use module::Module;
