//! The host's view of one guest call into a registered host function.

use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::internal::HostCallInternal;
use crate::resource::{ResourceHandle, ResourceHandleParts, ResourceTableRuntime, ResourceTypeId};
use crate::store::StoreContext;
use crate::store::StoreContextInternalExt;
use crate::types::ValueType;

/// The context a registered host function runs against, scoped to
/// one call from the guest.
///
/// Three surfaces are reachable: the host data of the [`Store<T>`]
/// through [`Self::data`] and [`Self::data_mut`], which mirror
/// [`Store::data`] and [`Store::data_mut`]; a mint entry point,
/// [`Self::resource_new`], that creates a fresh handle for a resource
/// type the calling instance knows; and the borrow of the store the
/// call runs against, through [`Self::store`], which is the value
/// the polyfill itself blocks a call on and starts a host `async`
/// function's task from.
///
/// The store surface is a [`StoreContext`], the polyfill's own view
/// of the store. The whole store sits behind that view — the
/// scheduler, the queues, the host tasks, the handle tables, and the
/// core store the runtime layer gave it — because of how a
/// trampoline is served: a suspended guest thread resumes outside
/// any poll of a driver, so the scheduler's state has to be
/// reachable from a trampoline that holds nothing but the context
/// the runtime layer handed it, and the polyfill's state therefore
/// rides in the core store's data. None of it is reachable from
/// here. What a [`StoreContext`] offers a host function is its host
/// data; the bookkeeping behind it is reached through a seam that
/// only the crate can name. The host data goes through:
///
/// ```rust
/// # use wasm_component_model_polyfill::{Engine, HostCall, Linker, Result};
/// let engine = Engine::new().unwrap();
/// let mut linker: Linker<u32> = Linker::new(&engine);
/// linker
///     .root()
///     .func_wrap("f", |mut call: HostCall<'_, u32>, (): ()| -> Result<()> {
///         *call.store().data_mut() += 1;
///         Ok(())
///     });
/// ```
///
/// The core store behind it does not:
///
/// ```compile_fail
/// # use wasm_component_model_polyfill::{Engine, HostCall, Linker, Result};
/// let engine = Engine::new().unwrap();
/// let mut linker: Linker<u32> = Linker::new(&engine);
/// linker
///     .root()
///     .func_wrap("f", |mut call: HostCall<'_, u32>, (): ()| -> Result<()> {
///         let _ = call.store().runtime_mut();
///         Ok(())
///     });
/// ```
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

impl<'a, T: 'static> HostCallInternal<'a, T> for HostCall<'a, T> {
    fn new(store: StoreContext<'a, T>, resource_tables: Vec<Option<ResourceTableRuntime>>) -> Self {
        HostCall {
            store,
            resource_tables,
        }
    }
}

impl<'a, T: 'static> HostCall<'a, T> {
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
    /// A host function that only reads or writes the host data
    /// reaches it through [`Self::data`] and [`Self::data_mut`] and
    /// never needs this. What this hands back is the same borrow the
    /// polyfill blocks a call on and starts a host `async` function's
    /// task from: the runtime layer hands a trampoline the core
    /// store's context and nothing else, so this is the store as the
    /// call reaches it, built from that context alone, because
    /// everything the polyfill carries rides in the core store's
    /// data. The bookkeeping that rides there is the crate's own and
    /// is not reachable through this value.
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
            // The refusal names the `own<T>` at issue, as every
            // unregistered-resource-type failure does: the type the
            // host asked to mint against, under the name the store
            // knows the identity by. An identity the store never
            // learned a name for — one no registration and no
            // instantiation of this store introduced — names
            // nothing, because there is no name to give it.
            return Err(Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: self
                    .store
                    .internal_ref()
                    .resource_type(type_id)
                    .map(ValueType::Own),
                cause: AbiCause::UnregisteredResourceType,
            }));
        };
        let mut guard = self.store.internal_ref().lock_tables()?;
        let table = guard.host_table(type_id);
        let index = guard.insert_own(table, type_id, known.guest_defined, rep);
        Ok(ResourceHandleParts {
            type_id,
            index,
            rep,
        }
        .into())
    }
}
