//! The canonical-ABI runtime state of one instantiation.
//!
//! A `canon lift` or `canon lower` names its memory, its
//! `cabi_realloc`, its `post-return`, and its callback by index into
//! slabs the executor's `Extract*` directives fill during
//! instantiation. This is those slabs, with the resource tables,
//! the component instances, and the may-leave flags the same
//! instantiation produced.
//!
//! The state lives under [`crate::abi`] because the memory and the
//! `cabi_realloc` it holds are the guest's, and the lift and lower
//! code is the only code that turns a slot into a read, a write, or
//! an allocation. A call site hands
//! [`BoundaryOptions`](crate::abi::options::BoundaryOptions) the
//! canon options and gets a resolved crossing back.

use std::sync::Arc;

use wasm_runtime_layer::{Func as RuntimeFunc, Memory};

use crate::abi::instance_flags::InstanceFlags;
use crate::concurrency::InstanceId;
use crate::resource::{ResourceTableRuntime, TableId};

/// Per-component canonical-ABI runtime state. Populated by the
/// executor's `Extract*` directives during instantiation; consulted
/// by a crossing at call time. Shared via `Arc<Mutex<...>>` so the
/// runtime layer's `Send + Sync` bound on `Func::new` is satisfied.
pub struct AbiRuntimeState {
    /// Every memory the instantiation extracted, by runtime slot.
    pub memories: Vec<Option<Memory>>,
    /// Every `cabi_realloc` the instantiation extracted, by runtime
    /// slot.
    pub reallocs: Vec<Option<RuntimeFunc>>,
    /// Every `post-return` the instantiation extracted, by runtime
    /// slot.
    pub post_returns: Vec<Option<RuntimeFunc>>,
    /// Every callback the instantiation extracted, by runtime slot.
    /// The callback of an export lifted `canon lift async (callback
    /// ...)` is resumed once per event the export's task receives.
    pub callbacks: Vec<Option<RuntimeFunc>>,
    /// Every resource table of the instance, by the translator's
    /// table index: the table created for this instantiation, the
    /// identity of the resource type it holds, and whether the table's
    /// instance defines the resource. `None` for an abstract table.
    /// Fixed once the instantiation has built it, so every crossing
    /// shares it rather than copying it.
    pub resource_tables: Arc<[Option<ResourceTableRuntime>]>,
    /// The store-wide identity of every component instance of this
    /// instantiation, by the translator's per-instantiation index.
    /// An adapter names its caller and its callee by that index; the
    /// enter intrinsic maps it onto the instance record.
    pub component_instances: Vec<InstanceId>,
    /// The handle table of every component instance of this
    /// instantiation, by the same index. An instance keeps one table
    /// shared by every handle kind it uses, so a waitable-set entry
    /// and a resource entry of one instance name the same table.
    pub handle_tables: Vec<TableId>,
    /// The may-leave flag of every component instance of this
    /// instantiation, by the same index. It is the core global the
    /// instance's adapters import, so a built-in that reads it here
    /// reads exactly what the generated code reads. Empty for a
    /// state built without an instantiation behind it, which is what
    /// a unit test of one crossing builds.
    pub instance_flags: Vec<InstanceFlags>,
}

impl AbiRuntimeState {
    /// Construct a state with the requested slab sizes, every slot
    /// initially empty.
    pub fn with_slabs(
        num_memories: usize,
        num_reallocs: usize,
        num_post_returns: usize,
        num_callbacks: usize,
        resource_tables: Vec<Option<ResourceTableRuntime>>,
        component_instances: Vec<InstanceId>,
        handle_tables: Vec<TableId>,
    ) -> Self {
        Self {
            memories: vec![None; num_memories],
            reallocs: vec![None; num_reallocs],
            post_returns: vec![None; num_post_returns],
            callbacks: vec![None; num_callbacks],
            resource_tables: resource_tables.into(),
            component_instances,
            handle_tables,
            instance_flags: Vec::new(),
        }
    }

    /// Give the state the may-leave flags of the instantiation's
    /// component instances, by the index the translator names an
    /// instance with.
    pub fn with_instance_flags(mut self, flags: Vec<InstanceFlags>) -> Self {
        self.instance_flags = flags;
        self
    }

    /// The may-leave flag of the component instance at `index`, the
    /// index the translator names an instance with.
    pub fn flags_at(&self, index: usize) -> Option<&InstanceFlags> {
        self.instance_flags.get(index)
    }

    /// The may-leave flag of the component instance `id`. A built-in
    /// whose crossing belongs to a task takes the instance off the
    /// task, which names it store-wide rather than by the
    /// translator's index.
    pub fn flags_of(&self, id: InstanceId) -> Option<&InstanceFlags> {
        let index = self
            .component_instances
            .iter()
            .position(|held| *held == id)?;
        self.flags_at(index)
    }
}
