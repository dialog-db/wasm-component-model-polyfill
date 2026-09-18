//! The component instance one boundary crossing belongs to.
//!
//! The reference builds the context of a crossing from three things
//! — the canon options, the component instance, and the borrow scope
//! — and reads the handle table it resolves a handle against off the
//! instance. This type is that instance as the polyfill holds it:
//! the store-wide identity of the component instance, the store's
//! handle tables, and the resource tables of the instance. A
//! crossing that carries an `own<T>` or a `borrow<T>` reaches both
//! tables through here, so the context of the crossing takes the
//! instance and derives the tables from it rather than taking each
//! table separately.
//!
//! The options and the instance of a crossing come out of the same
//! runtime state, so [`BoundaryInstance::resolve`] reads both under
//! one lock of it.

use std::sync::{Arc, Mutex};

use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::InstanceId;
use crate::error::{Error, Result};
use crate::executor::ir::CanonOptions;
use crate::resource::{HandleTables, ResourceTableRuntime};

/// The component instance one crossing belongs to, with the tables
/// the crossing resolves its handles against.
#[derive(Clone)]
pub struct BoundaryInstance {
    /// The store-wide identity of the component instance. The
    /// instance record it names carries the entry gate, the
    /// backpressure counter, and the flags an asynchronous crossing
    /// consults.
    id: Option<InstanceId>,
    /// The per-store handle tables. Required when the crossing
    /// carries `own<T>` or `borrow<T>` valtypes; `None` is rejected
    /// at first contact.
    tables: Option<Arc<Mutex<HandleTables>>>,
    /// Every resource table of the component instance, by table
    /// index. A handle's declared type names the index; this maps it
    /// to the table the instance keeps and the resource it holds.
    resource_tables: Vec<Option<ResourceTableRuntime>>,
}

impl BoundaryInstance {
    /// The options and the instance of one crossing, read out of the
    /// instantiation's runtime state under a single lock of it: the
    /// slots the options name and the instance's resource tables sit
    /// in the same state, and a call site needs both before it can
    /// build a context.
    pub fn resolve(
        declared: &CanonOptions,
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
            resource_tables: state.resource_tables.clone(),
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
            resource_tables: Vec::new(),
        }
    }

    /// The same tables, addressed by the component instance `id`
    /// rather than the one the declared options name. A built-in
    /// whose crossing belongs to a task takes the instance off the
    /// task, which is the instance the reference builds its context
    /// from.
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

    /// Every resource table of the component instance, by table
    /// index.
    pub fn resource_tables(&self) -> &[Option<ResourceTableRuntime>] {
        &self.resource_tables
    }
}
