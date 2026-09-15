//! The host's view of one guest call into a registered host function.

use std::sync::{Arc, Mutex};

use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::resource::{HandleTables, ResourceHandle, ResourceTypeId};
use crate::types::{ResourceType, ValueType};

/// The context a registered host function runs against, scoped to
/// one call from the guest.
///
/// Two surfaces are reachable: the host data of the [`Store<T>`]
/// through [`Self::data`] and [`Self::data_mut`], which mirror
/// [`Store::data`] and [`Store::data_mut`], and a mint entry point,
/// [`Self::resource_new`], that creates a fresh handle for a resource
/// type the calling instance knows. No runtime layer type appears
/// here: the context is a borrowed view onto the polyfill's own
/// store state.
///
/// [`Store<T>`]: crate::Store
/// [`Store::data`]: crate::Store::data
/// [`Store::data_mut`]: crate::Store::data_mut
pub struct HostCall<'a, T> {
    data: &'a mut T,
    tables: Arc<Mutex<HandleTables>>,
    /// The identities of the resource types of the instance whose
    /// import is being served, by resource index. A mint against
    /// any other identity is refused.
    resource_types: Vec<ResourceTypeId>,
}

impl<'a, T> HostCall<'a, T> {
    /// Construct the context for one call.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(
        data: &'a mut T,
        tables: Arc<Mutex<HandleTables>>,
        resource_types: Vec<ResourceTypeId>,
    ) -> Self {
        Self {
            data,
            tables,
            resource_types,
        }
    }

    /// The store's host data.
    pub fn data(&self) -> &T {
        self.data
    }

    /// The store's host data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.data
    }

    /// Mint a fresh handle for the resource type `type_id`, with
    /// `rep` as its representation. The handle is live in the store's
    /// table when the guest reads it, so the closure can return it
    /// through an `own<T>` result. A `type_id` that is not one of the
    /// calling instance's resource types is refused with the
    /// unregistered-resource-type ABI cause.
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        if !self.resource_types.contains(&type_id) {
            return Err(Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: ValueType::Own(ResourceType::new("resource")),
                cause: AbiCause::UnregisteredResourceType,
            }));
        }
        let mut guard = self
            .tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let index = guard.for_type_mut(type_id).insert(rep);
        Ok(ResourceHandle {
            type_id,
            index,
            rep,
        })
    }
}
