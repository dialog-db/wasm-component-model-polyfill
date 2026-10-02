// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A module as a backend compiled it.

use core::any::Any;

use crate::contract::{MaybeSend, MaybeSync};
use crate::module::{ExportType, ImportType};

/// A compiled module, as the backend that compiled it holds it.
///
/// The module describes its boundary and nothing else. The types, globals,
/// tables, and tags inside it belong to the engine, and none of them is a
/// reason for a backend to refuse the module.
pub trait BackendModule: MaybeSend + MaybeSync + 'static {
    /// The imports of the module, in the order an instantiation takes them.
    fn imports(&self) -> &[ImportType];

    /// The exports of the module, in the order the module declares them.
    fn exports(&self) -> &[ExportType];

    /// The module as [`Any`], so the store of the same backend can reach its
    /// own type when it instantiates the module.
    fn as_any(&self) -> &dyn Any;
}
