//! The component instance one boundary crossing belongs to.
//!
//! The reference builds the context of a crossing from three things
//! — the canon options, the component instance, and the borrow scope
//! — and reads the handle table it resolves a handle against off the
//! instance. This type is that instance as the polyfill holds it:
//! the store-wide identity of the component instance, the store's
//! handle tables, the resource tables of the instantiation, and the
//! instance's may-leave flag. A crossing that carries an `own<T>` or
//! a `borrow<T>` reaches both tables through here, so the context of
//! the crossing takes the instance and derives the tables from it
//! rather than taking each table separately. A call the polyfill
//! makes into the guest clears the flag it finds here for the length
//! of the call.
//!
//! The options and the instance of a crossing come out of the same
//! runtime state, so [`BoundaryInstance::resolve`] reads both under
//! one lock of it.

use std::sync::{Arc, Mutex};

use crate::abi::instance_flags::InstanceFlags;
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::InstanceId;
use crate::error::{Error, Result};
use crate::executor::ir::CanonOptions;
use crate::internal::ErrorInternal;
use crate::resource::{HandleTables, ResourceTableRuntime};

/// The component instance one crossing belongs to, with the tables
/// the crossing resolves its handles against.
#[derive(Clone)]
pub struct BoundaryInstance {
    /// The store-wide identity of the component instance. The
    /// instance record it names carries the entry gate, the
    /// backpressure counter, and the suspend flag an asynchronous
    /// crossing consults.
    id: Option<InstanceId>,
    /// The per-store handle tables. Required when the crossing
    /// carries `own<T>` or `borrow<T>` valtypes; `None` is rejected
    /// at first contact.
    tables: Option<Arc<Mutex<HandleTables>>>,
    /// Every resource table of the instantiation, by table index. A
    /// handle's declared type names the index; this maps it to the
    /// table the instantiation keeps and the resource it holds. The
    /// vector is the instantiation's own, shared rather than copied;
    /// `None` for a crossing that names no instantiation.
    resource_tables: Option<Arc<[Option<ResourceTableRuntime>]>>,
    /// The may-leave flag of the component instance, which is the
    /// core global its adapters compile against. A call the polyfill
    /// makes into the guest clears it for the length of the call.
    /// `None` for a crossing that names no component instance of an
    /// instantiation, which is a copy between two guest memories.
    flags: Option<InstanceFlags>,
}

impl BoundaryInstance {
    /// The options and the instance of one crossing, read out of the
    /// instantiation's runtime state under a single lock of it: the
    /// slots the options name and the instantiation's resource
    /// tables sit in the same state, and a call site needs both
    /// before it can build a context.
    pub fn resolve(
        declared: &Arc<CanonOptions>,
        abi_state: &Arc<Mutex<AbiRuntimeState>>,
        tables: &Arc<Mutex<HandleTables>>,
    ) -> Result<(BoundaryOptions, Self)> {
        let state = abi_state
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
        let options = BoundaryOptions::from_state(declared, &state);
        let instance = Self {
            id: options.instance(),
            tables: Some(tables.clone()),
            resource_tables: Some(state.resource_tables.clone()),
            flags: state.flags_at(declared.instance).cloned(),
        };
        Ok((options, instance))
    }

    /// The instance of a crossing that carries no handles of its
    /// own, which is a copy between two guest memories: an adapter's
    /// transcoder names the two memories and nothing else, and the
    /// instance it belongs to is the one the task on the stack
    /// belongs to.
    pub fn without_tables(id: Option<InstanceId>) -> Self {
        Self {
            id,
            tables: None,
            resource_tables: None,
            flags: None,
        }
    }

    /// The same tables, addressed by the component instance `id`
    /// rather than the one the declared options name. A built-in
    /// whose crossing belongs to a task takes the instance off the
    /// task, which is the instance the reference builds its context
    /// from.
    ///
    /// The resource tables and the may-leave flag stay as
    /// [`Self::resolve`] read them, and there is nothing to
    /// re-resolve them from: the tables are the instantiation's,
    /// indexed by the translator's table index, and one
    /// instantiation has one such vector however many component
    /// instances it holds, while the flag is the one global the
    /// options' instance owns. The `id` and the instance the options
    /// name also address the same component instance in every
    /// component the translator accepts today, because a built-in is
    /// reachable only from the core modules of the instance whose
    /// definition declares it, so the flag that travels here is that
    /// instance's either way.
    pub fn with_id(mut self, id: InstanceId) -> Self {
        self.id = Some(id);
        self
    }

    /// The store-wide identity of the component instance.
    pub fn id(&self) -> Option<InstanceId> {
        self.id
    }

    /// The per-store handle tables the crossing resolves handles
    /// against.
    pub fn tables(&self) -> Option<&Arc<Mutex<HandleTables>>> {
        self.tables.as_ref()
    }

    /// Every resource table of the instantiation, by table index.
    pub fn resource_tables(&self) -> &[Option<ResourceTableRuntime>] {
        self.resource_tables.as_deref().unwrap_or(&[])
    }

    /// The instantiation's resource tables as the shared vector, for
    /// a host call that keeps them for its length. Empty for a
    /// crossing that names no instantiation.
    pub fn shared_resource_tables(&self) -> Arc<[Option<ResourceTableRuntime>]> {
        self.resource_tables
            .clone()
            .unwrap_or_else(|| Arc::from([]))
    }

    /// The may-leave flag of the component instance.
    pub fn flags(&self) -> Option<&InstanceFlags> {
        self.flags.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ir::{DataModel, StringEncoding};
    use crate::internal::ResourceTypeIdInternal;
    use crate::resource::{ResourceTypeId, TableId};

    #[wcmp_macros::test]
    fn it_shares_the_instantiations_resource_tables_with_every_crossing() {
        // The resource tables are fixed once the instantiation has
        // built them, and every crossing reads them. Resolving a
        // crossing hands it the instantiation's own vector, so two
        // crossings — and the host call one of them serves — read
        // the same one rather than a copy each.
        let table = ResourceTableRuntime {
            table: TableId::fresh(),
            type_id: ResourceTypeId::fresh(),
            resource_index: 0,
            defining: false,
            guest_defined: false,
        };
        let abi_state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
            0,
            0,
            0,
            0,
            vec![Some(table)],
            vec![InstanceId::from_index(0)],
            vec![TableId::fresh()],
        )));
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let declared = Arc::new(CanonOptions {
            instance: 0,
            memory: None,
            realloc: None,
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        });

        let (_, first) =
            BoundaryInstance::resolve(&declared, &abi_state, &tables).expect("resolve");
        let (_, second) =
            BoundaryInstance::resolve(&declared, &abi_state, &tables).expect("resolve");
        let held = abi_state.lock().expect("state").resource_tables.clone();

        assert_eq!(first.resource_tables().len(), 1);
        assert!(
            core::ptr::eq(first.resource_tables(), &*held)
                && core::ptr::eq(second.resource_tables(), &*held)
                && Arc::ptr_eq(&first.shared_resource_tables(), &held),
            "every crossing reads the instantiation's one vector"
        );
    }
}
