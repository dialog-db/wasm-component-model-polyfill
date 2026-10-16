// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The typed handle to a host resource, as Wasmtime's `Resource<T>`
//! (`crates/wasmtime/src/runtime/component/resources/host_static.rs`
//! at `v49.0.0-rc.1`).

use core::fmt;
use core::marker::PhantomData;

use crate::error::Result;
use crate::internal::{ResourceAnyInternal, ResourceInternal, ResourceTypeInternal};
use crate::store::StoreContext;
use crate::types::ResourceType;

use super::host_handle::HostHandle;
use super::resource_any::{ResourceAny, resource_type_mismatch};

/// A handle to a host resource of the identity
/// [`ResourceType::host::<T>()`](ResourceType::host).
///
/// `T` is a marker that names the resource type: the resource itself
/// is the host's own, and the handle carries only its `u32` rep. The
/// handle is `Send` and `Sync` whatever `T` is.
///
/// A handle is an `own` or a `borrow`. An owned handle is the host's
/// until it is lowered into a guest as an `own`, which moves the
/// resource into the guest: a second lowering as an `own` then fails
/// with "host resource already consumed". Lowering an owned handle
/// as a `borrow` puts it in the store's host table, where each borrow
/// of it is tracked. A borrow is one the host made with
/// [`Resource::new_borrow`] or received as a `borrow` parameter, and
/// lowers only as a `borrow`.
///
/// The handle crosses where the component declares an `own` or a
/// `borrow` of this identity, and nowhere else: a resource a
/// component defines never crosses as one.
///
/// The handle is not `Clone`, because ownership moves with it. A
/// resource that more than one owner holds goes through
/// [`ResourceAny`] instead.
pub struct Resource<T: 'static> {
    handle: HostHandle,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> Resource<T> {
    /// An owned handle of the resource `rep`.
    pub fn new_own(rep: u32) -> Self {
        Self::from_handle(HostHandle::own(rep))
    }

    /// A borrow of the resource `rep`.
    pub fn new_borrow(rep: u32) -> Self {
        Self::from_handle(HostHandle::borrow(rep))
    }

    /// The resource's `u32` rep.
    pub fn rep(&self) -> u32 {
        self.handle.rep()
    }

    /// Whether the handle is owned, and not a borrow.
    pub fn owned(&self) -> bool {
        self.handle.owned()
    }

    /// The handle of `resource`, whose type must be
    /// [`ResourceType::host::<T>()`](ResourceType::host). An owned
    /// resource leaves the store's host table, and a borrow keeps its
    /// entry, which goes when the call that lent it ends.
    ///
    /// # Errors
    ///
    /// A resource of another type fails with "resource type mismatch",
    /// and one the host table no longer holds fails with the
    /// invalid-handle cause.
    pub fn try_from_resource_any<D: 'static>(
        resource: ResourceAny,
        store: &mut StoreContext<'_, D>,
    ) -> Result<Self> {
        resource
            .into_host(store, ResourceType::host::<T>())
            .map(Self::from_handle)
    }

    /// The [`ResourceAny`] of this handle, which names an entry of the
    /// store's host table. An owned handle in no table takes an entry,
    /// and a borrow takes one that goes when the call in flight ends.
    ///
    /// The entry has no destructor: releasing the [`ResourceAny`] with
    /// [`ResourceAny::resource_drop`] runs none, as in Wasmtime.
    ///
    /// # Errors
    ///
    /// A handle already lowered into a guest as an `own` fails with
    /// "host resource already consumed", and a borrow outside a call
    /// fails with the invalid-handle cause.
    pub fn try_into_resource_any<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
    ) -> Result<ResourceAny> {
        ResourceAny::from_host(store, &self.handle, ResourceType::host::<T>())
    }

    fn from_handle(handle: HostHandle) -> Self {
        Self {
            handle,
            _marker: PhantomData,
        }
    }
}

impl<T: 'static> ResourceInternal for Resource<T> {
    fn handle(&self) -> &HostHandle {
        &self.handle
    }

    fn from_lift(handle: HostHandle, identity: ResourceType) -> Result<Self> {
        if identity.is_host::<T>() {
            Ok(Self::from_handle(handle))
        } else {
            Err(resource_type_mismatch(identity))
        }
    }
}

impl<T: 'static> fmt::Debug for Resource<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resource")
            .field("rep", &self.handle.rep())
            .field("state", &self.handle.state())
            .finish()
    }
}
