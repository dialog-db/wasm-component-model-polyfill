// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resource-handle support: identity, per-store handle tables, and
//! the host-resource registration carrier.
//!
//! The polyfill realises the canonical-ABI's runtime-state rules for
//! `own<T>` and `borrow<T>` against the types in this module:
//!
//! - [`ResourceTypeId`] is the engine-issued identity that names a
//!   registered resource type. Every host registration mints one.
//! - [`HandleTable`] is one slab the canonical ABI's
//!   index-allocation and reuse rules govern, and [`HandleTables`]
//!   is the store's collection of them.
//! - [`TaskEnd`] is what one attempt to end a task achieved, which
//!   is what [`HandleTables`] answers the callers of its task exits
//!   with.
//! - [`ResourceHandle`] is the polyfill's opaque addressing surface
//!   for handles that pass through [`Val::Own`] and [`Val::Borrow`].
//!
//! Host-resource registration itself lives on
//! [`LinkerInstance`]; the carrier type a [`Linker`] stores per
//! interface lives in [`super::linker`].
//!
//! [`Linker`]: crate::Linker
//! [`LinkerInstance`]: crate::LinkerInstance
//! [`Val::Own`]: crate::Val::Own
//! [`Val::Borrow`]: crate::Val::Borrow

mod handle;
mod handle_kind;
mod handle_lookup_error;
mod handle_parts;
mod identity;
mod table;
mod table_id;
mod table_runtime;
mod tables;
mod task_end;

pub use handle::ResourceHandle;
pub use handle_kind::HandleKind;
pub use handle_lookup_error::HandleLookupError;
pub use handle_parts::ResourceHandleParts;
pub use identity::ResourceTypeId;
pub use table_id::TableId;
pub use table_runtime::ResourceTableRuntime;
pub use tables::HandleTables;
pub use task_end::TaskEnd;
