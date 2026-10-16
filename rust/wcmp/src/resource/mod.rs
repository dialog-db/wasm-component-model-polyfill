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
//! - [`ResourceType`] is the identity of a resource type, which
//!   every table and every handle carries.
//! - [`HandleTable`] is one slab the canonical ABI's
//!   index-allocation and reuse rules govern, and [`HandleTables`]
//!   is the store's collection of them.
//! - [`TaskEnd`] is what one attempt to end a task achieved, which
//!   is what [`HandleTables`] answers the callers of its task exits
//!   with.
//! - [`Resource<T>`] and [`ResourceDynamic`] are the host's typed
//!   handles to a host resource, and [`ResourceAny`] is the handle to
//!   a resource of any type, which [`Val::Resource`] carries.
//!
//! Host-resource registration itself lives on
//! [`LinkerInstance`]; the carrier type a [`Linker`] stores per
//! interface lives in [`super::linker`].
//!
//! [`Linker`]: crate::Linker
//! [`LinkerInstance`]: crate::LinkerInstance
//! [`Val::Resource`]: crate::Val::Resource
//! [`ResourceType`]: crate::ResourceType

mod handle_kind;
mod handle_lookup_error;
mod host_dynamic;
mod host_handle;
mod host_handle_state;
mod host_static;
mod resource_any;
mod resource_table;
mod table;
mod table_id;
mod table_runtime;
mod tables;
mod task_end;

pub use handle_kind::HandleKind;
pub use handle_lookup_error::HandleLookupError;
pub use host_dynamic::ResourceDynamic;
pub use host_handle::HostHandle;
pub use host_handle_state::HostHandleState;
pub use host_static::Resource;
pub use resource_any::{CONSUMED, ResourceAny, host_reason};
pub use resource_table::ResourceTable;
pub use table_id::TableId;
pub use table_runtime::ResourceTableRuntime;
pub use tables::HandleTables;
pub use task_end::TaskEnd;
