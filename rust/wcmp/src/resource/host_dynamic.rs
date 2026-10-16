// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The handle to a host resource of a type the host makes at run
//! time, as Wasmtime's `ResourceDynamic`
//! (`crates/wasmtime/src/runtime/component/resources/host_dynamic.rs`
//! at `v49.0.0-rc.1`).

use crate::error::Result;
use crate::internal::{ResourceAnyInternal, ResourceInternal, ResourceTypeInternal};
use crate::store::StoreContext;
use crate::types::ResourceType;

use super::host_handle::HostHandle;
use super::resource_any::{ResourceAny, resource_type_mismatch};

/// A handle to a host resource of the identity
/// [`ResourceType::host_dynamic(ty)`](ResourceType::host_dynamic),
/// for a host that makes its resource types at run time.
///
/// It is [`Resource<T>`](super::Resource) with the `u32` of the type
/// in place of the marker `T`, and moves, lowers, and lifts as that
/// does. It crosses where the component declares an `own` or a
/// `borrow` of any dynamic host type, and the lowering checks that the
/// declared type is the handle's own.
#[derive(Debug)]
pub struct ResourceDynamic {
    handle: HostHandle,
    ty: u32,
}

impl ResourceDynamic {
    /// An owned handle of the resource `rep`, of the dynamic type `ty`.
    pub fn new_own(rep: u32, ty: u32) -> Self {
        Self {
            handle: HostHandle::own(rep),
            ty,
        }
    }

    /// A borrow of the resource `rep`, of the dynamic type `ty`.
    pub fn new_borrow(rep: u32, ty: u32) -> Self {
        Self {
            handle: HostHandle::borrow(rep),
            ty,
        }
    }

    /// The resource's `u32` rep.
    pub fn rep(&self) -> u32 {
        self.handle.rep()
    }

    /// The `u32` of the handle's dynamic type.
    pub fn ty(&self) -> u32 {
        self.ty
    }

    /// Whether the handle is owned, and not a borrow.
    pub fn owned(&self) -> bool {
        self.handle.owned()
    }

    /// The handle of `resource`, whose type must be a dynamic host
    /// type, as [`Resource::try_from_resource_any`] makes one.
    ///
    /// [`Resource::try_from_resource_any`]: super::Resource::try_from_resource_any
    ///
    /// # Errors
    ///
    /// A resource of a type that is not a dynamic host type fails
    /// with "resource type mismatch", and one the host table no
    /// longer holds fails with the invalid-handle cause.
    pub fn try_from_resource_any<D: 'static>(
        resource: ResourceAny,
        store: &mut StoreContext<'_, D>,
    ) -> Result<Self> {
        let Some(ty) = resource.ty().as_host_dynamic() else {
            return Err(resource_type_mismatch(resource.ty()));
        };
        let handle = resource.into_host(store, ResourceType::host_dynamic(ty))?;
        Ok(Self { handle, ty })
    }

    /// The [`ResourceAny`] of this handle, as
    /// [`Resource::try_into_resource_any`] makes one.
    ///
    /// [`Resource::try_into_resource_any`]: super::Resource::try_into_resource_any
    ///
    /// # Errors
    ///
    /// As [`Resource::try_into_resource_any`].
    pub fn try_into_resource_any<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
    ) -> Result<ResourceAny> {
        ResourceAny::from_host(store, &self.handle, ResourceType::host_dynamic(self.ty))
    }
}

impl ResourceInternal for ResourceDynamic {
    fn handle(&self) -> &HostHandle {
        &self.handle
    }

    fn from_lift(handle: HostHandle, identity: ResourceType) -> Result<Self> {
        match identity.as_host_dynamic() {
            Some(ty) => Ok(Self { handle, ty }),
            None => Err(resource_type_mismatch(identity)),
        }
    }
}
