// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Engine-issued opaque identity for a registered resource type.
//!
//! A `ResourceTypeId` is the unique key the polyfill uses to address
//! a host-registered resource type. The id is minted by the
//! [`Engine`] at registration time and threaded through the
//! polyfill's runtime state so handles minted against one
//! registration cannot collide with handles minted against another
//! — even when the two registrations share the same label.
//!
//! Identity is workspace-internal: the [`Engine`] hands one out
//! on demand, every other module compares ids by value, and
//! consumers never see the underlying integer or the mint that
//! issued it.
//!
//! [`Engine`]: crate::Engine

use std::sync::atomic::{AtomicU64, Ordering};

use crate::internal::ResourceTypeIdInternal;

/// An engine-issued unique identity for a registered resource type.
///
/// Two `ResourceTypeId` values compare equal only when they came
/// from the same minting. The wrapped integer is opaque and not
/// exposed, and neither is the minting: the store keys a resource
/// type's handle table and its destructor by the identity, so an
/// identity a caller outside had minted for itself would name an
/// entry some registration already answers to. Minting is therefore
/// crate-internal: a caller outside receives an identity from the
/// registration that minted it — [`LinkerInstance::resource`],
/// [`LinkerInstance::resource_with`], [`HostResource::type_id`] — and
/// has no way to make one of its own.
///
/// The identity imports and compares:
///
/// ```rust
/// use wcmp::{HostResource, ResourceTypeId};
/// let resource = HostResource::new(|_: &mut (), _: u32| Ok(()));
/// let id: ResourceTypeId = resource.type_id();
/// assert_eq!(id, resource.clone().type_id());
/// ```
///
/// Minting one does not:
///
/// ```compile_fail
/// use wcmp::ResourceTypeId;
/// let _ = ResourceTypeId::fresh();
/// ```
///
/// [`LinkerInstance::resource`]: crate::LinkerInstance::resource
/// [`LinkerInstance::resource_with`]: crate::LinkerInstance::resource_with
/// [`HostResource::type_id`]: crate::HostResource::type_id
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceTypeId(u64);

impl ResourceTypeIdInternal for ResourceTypeId {
    /// The counter is process-wide; collisions across engines are
    /// not possible while the process is alive.
    fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
