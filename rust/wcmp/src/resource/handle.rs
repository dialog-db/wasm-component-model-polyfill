// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The polyfill's opaque handle into one of a store's handle tables.
//!
//! A `ResourceHandle` is the value the polyfill exposes through the
//! [`Val::Own`] and [`Val::Borrow`] variants. It carries the
//! resource-type identity the handle was minted against alongside
//! the handle-table index that names the live entry.
//!
//! Identity comparison is structural: two handles compare equal when
//! their resource type, index, rep, and the generation of the entry
//! they were minted for all match, so a handle kept past its entry's
//! release never equals the handle of the entry that took its index. A
//! handle is cheaply cloneable; copying does not duplicate the
//! underlying table entry.
//!
//! [`Val::Own`]: crate::Val::Own
//! [`Val::Borrow`]: crate::Val::Borrow

use super::handle_parts::ResourceHandleParts;
use super::identity::ResourceTypeId;
use crate::internal::ResourceHandleInternal;

/// An opaque handle into one of a store's handle tables.
///
/// The polyfill mints these when a host registers a resource against
/// a [`LinkerInstance`] and hands one across the canonical-ABI
/// boundary, or when a guest produces one during a lifted call. Every
/// handle the host holds names an entry of the host's per-resource-type
/// table in the [`Store`]: an `own<T>` the host owns outright, or a
/// `borrow<T>` it received out of a guest, whose entry goes when the
/// call that lent it ends.
///
/// The parts are readable and not writable. A handle names a live
/// entry in a table the store owns, so only the store mints one: it
/// is built from `ResourceHandleParts`, which `lib.rs` never
/// re-exports, and host code that assembled a handle of its own
/// would address an entry the store never gave it. A host reads the
/// parts through [`Self::type_id`], [`Self::index`], and
/// [`Self::rep`], and writes none of them:
///
/// ```rust
/// use wcmp::ResourceHandle;
/// fn parts(handle: &ResourceHandle) -> (u32, u32) {
///     (handle.index(), handle.rep())
/// }
/// ```
///
/// ```compile_fail
/// # use wcmp::{Engine, ResourceHandle, ResourceTypeId, Store};
/// # fn forge(type_id: ResourceTypeId) -> ResourceHandle {
/// ResourceHandle {
///     type_id,
///     index: 999,
///     rep: 0,
/// }
/// # }
/// ```
///
/// The parts type is the same rule stated once more, for the entry
/// that builds a handle rather than for the fields. The crate builds
/// one through [`From<ResourceHandleParts>`][From], and `lib.rs`
/// re-exports the handle and not its parts, so naming the parts at
/// all is what fails — not the conversion, which is a perfectly
/// ordinary `impl`:
///
/// ```compile_fail
/// use wcmp::{ResourceHandle, ResourceHandleParts};
/// fn forge(parts: ResourceHandleParts) -> ResourceHandle {
///     parts.into()
/// }
/// ```
///
/// [`LinkerInstance`]: crate::LinkerInstance
/// [`Store`]: crate::Store
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceHandle {
    /// The engine-issued identity of the resource type this handle
    /// addresses.
    type_id: ResourceTypeId,
    /// The handle-table entry this handle names.
    index: u32,
    /// The resource's 32-bit representation, as the host's table
    /// entry holds it. Carried so that a lower can tell a handle from
    /// one that names another entry at the same index and generation,
    /// which only a handle the host did not get from this table can.
    rep: u32,
    /// The generation of the host-table entry the handle was minted
    /// for. An index the host's table frees and gives to another entry
    /// takes a new generation, so a handle kept past its entry's
    /// release is told from the entry that took its index.
    generation: u32,
}

impl ResourceHandle {
    /// The identity of the resource type this handle addresses.
    pub fn type_id(&self) -> ResourceTypeId {
        self.type_id
    }

    /// The handle-table entry this handle names.
    ///
    /// The index is meaningful only against the table the handle
    /// came from, which the handle itself does not name.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The resource's 32-bit representation: what the host supplied
    /// when it minted the handle, or what a lift read out of the
    /// guest's table entry.
    pub fn rep(&self) -> u32 {
        self.rep
    }
}

impl ResourceHandleInternal for ResourceHandle {
    fn generation(&self) -> u32 {
        self.generation
    }
}

impl From<ResourceHandleParts> for ResourceHandle {
    fn from(parts: ResourceHandleParts) -> Self {
        Self {
            type_id: parts.type_id,
            index: parts.index,
            rep: parts.rep,
            generation: parts.generation,
        }
    }
}
