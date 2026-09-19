//! The host's view of one guest call into a registered host function.

use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::resource::{ResourceHandle, ResourceTableRuntime, ResourceTypeId};
use crate::store::StoreContext;

/// The context a registered host function runs against, scoped to
/// one call from the guest.
///
/// Three surfaces are reachable: the host data of the [`Store<T>`]
/// through [`Self::data`] and [`Self::data_mut`], which mirror
/// [`Store::data`] and [`Store::data_mut`]; a mint entry point,
/// [`Self::resource_new`], that creates a fresh handle for a resource
/// type the calling instance knows; and the store itself through
/// [`Self::store`], which is how a call that has to block reaches
/// the scheduler's suspend seam and how a host `async` function's
/// call starts its host task.
///
/// The store surface is a [`StoreContext`], the polyfill's own view
/// of the store, and the whole store sits behind it: the scheduler,
/// the queues, the host tasks, and the handle tables. That view is
/// also the wrapper around the core store's context, so
/// `call.store().runtime_mut()` reaches the runtime layer's own
/// store context in two hops. The reach follows from how a
/// trampoline is served rather than from anything this boundary
/// means to offer: a suspended guest thread resumes outside any
/// poll of a driver, so the scheduler's state has to be reachable
/// from a trampoline that holds nothing but the context the runtime
/// layer handed it, and the polyfill's state therefore rides in the
/// core store's data. What the context promises a host function is
/// the three surfaces above. The core store underneath them carries
/// the polyfill's own bookkeeping, and a host function that drives
/// it directly is outside what the polyfill defines.
///
/// [`Store<T>`]: crate::Store
/// [`Store::data`]: crate::Store::data
/// [`Store::data_mut`]: crate::Store::data_mut
pub struct HostCall<'a, T: 'static> {
    store: StoreContext<'a, T>,
    /// The resource tables of the instance whose import is being
    /// served. A mint against a resource type none of them holds is
    /// refused.
    resource_tables: Vec<Option<ResourceTableRuntime>>,
}

impl<'a, T: 'static> HostCall<'a, T> {
    /// Construct the context for one call.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(
        store: StoreContext<'a, T>,
        resource_tables: Vec<Option<ResourceTableRuntime>>,
    ) -> Self {
        Self {
            store,
            resource_tables,
        }
    }

    /// The store's host data.
    pub fn data(&self) -> &T {
        self.store.data()
    }

    /// The store's host data, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut()
    }

    /// The store this call runs against.
    ///
    /// A host function that only reads or writes the host data never
    /// needs this. A host function that blocks does: the scheduler's
    /// suspend seam takes the store the block runs against, and so
    /// does the host task of a host `async` function. The runtime
    /// layer hands a trampoline the core store's context and nothing
    /// else, so this is the store as the call reaches it — the same
    /// value a turn runs against, built from that context alone,
    /// because everything the polyfill carries rides in the core
    /// store's data.
    pub fn store(&mut self) -> &mut StoreContext<'a, T> {
        &mut self.store
    }

    /// Mint a fresh handle for the resource type `type_id`, with
    /// `rep` as its representation. The handle is live in the store's
    /// table when the guest reads it, so the closure can return it
    /// through an `own<T>` result. A `type_id` that is not one of the
    /// calling instance's resource types is refused with the
    /// unregistered-resource-type ABI cause.
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        let known = self
            .resource_tables
            .iter()
            .flatten()
            .find(|table| table.type_id == type_id)
            .copied();
        let Some(known) = known else {
            // The instance holds no resource type under this
            // identity, so there is no `own<T>` the refusal could
            // name: naming one would be naming the very type the
            // cause says does not exist.
            return Err(Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: None,
                cause: AbiCause::UnregisteredResourceType,
            }));
        };
        let mut guard = self.store.lock_tables()?;
        let table = guard.host_table(type_id);
        let index = guard.insert_own(table, type_id, known.guest_defined, rep);
        Ok(ResourceHandle {
            type_id,
            index,
            rep,
        })
    }
}
