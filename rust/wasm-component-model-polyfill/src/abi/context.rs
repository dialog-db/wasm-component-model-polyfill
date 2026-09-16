//! The boundary context: the one lift and lower crossing.
//!
//! A boundary context is the object through which one value crosses
//! between the host's [`Val`] and the guest's memory or flat slots.
//! One context is built per crossing, from three things: the canon
//! options of the lift or lower, the component instance, and the
//! task or subtask whose borrows and lends the crossing counts
//! against.
//!
//! The context is the only object that reads guest memory, writes
//! guest memory, or asks the guest for memory. A trampoline, an
//! intrinsic, or an export call hands it a value, a type, and a
//! position, and reads back a value or a list of flat slots.
//!
//! The context carries its options as a value, and it selects its
//! [`AbiStrategy`] from them when it is built, so a crossing under a
//! second strategy needs no change at the call site. It also
//! exposes the lift options and the result type of the task on the
//! stack, which a `task.return` compares its own against.
//!
//! Construction is workspace-internal — the context is always built
//! immediately before a call drives the canonical ABI.
//!
//! [`Val`]: crate::value::Val

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{StoreContextMut, Val as RuntimeVal};

use crate::abi::options::BoundaryOptions;
use crate::abi::strategy::AbiStrategy;
use crate::backend::Backend;
use crate::component::FunctionType;
use crate::concurrency::{InstanceId, Scope};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::{CanonOptions, StringEncoding};
use crate::resource::{HandleTables, ResourceTableRuntime};
use crate::types::ValueType;

/// The context of one canonical-ABI crossing.
///
/// The store context is held as a mutable [`StoreContextMut`] rather
/// than a reference to the polyfill's `Store` so the same context
/// type works for every crossing: an export call, a host
/// trampoline, and an adapter's intrinsics, of which only the first
/// holds the polyfill's `Store`.
pub struct BoundaryContext<'a, T: 'static> {
    /// The runtime-layer store context the crossing runs in.
    store: StoreContextMut<'a, T, Backend>,
    /// The options of the crossing: memory, realloc, string
    /// encoding, and data model.
    options: BoundaryOptions,
    /// The options of the side a copy between two guest memories
    /// reads from. `None` for every crossing that is not such a
    /// copy, which is every crossing between a host [`Val`] and a
    /// guest.
    ///
    /// [`Val`]: crate::value::Val
    source: Option<BoundaryOptions>,
    /// The component instance the crossing belongs to. A lift or a
    /// lower takes it from its options. A copy between two guest
    /// memories takes it from the task on the stack, because the
    /// fused adapter's transcoder names only the two memories, and
    /// has none when nothing is on the stack.
    ///
    /// Nothing reads it yet. The instance record it names carries
    /// the entry gate, the backpressure counter, and the flags that
    /// an asynchronous crossing consults.
    #[allow(dead_code)]
    instance: Option<InstanceId>,
    /// The task or subtask the crossing's borrows and lends count
    /// against. Absent only for a crossing driven with no call in
    /// flight, which no guest reaches.
    scope: Option<Scope>,
    /// The strategy the options selected.
    strategy: AbiStrategy,
    /// The per-store handle tables. Required when the crossing
    /// carries `own<T>` or `borrow<T>` valtypes; `None` is rejected
    /// at first contact.
    tables: Option<Arc<Mutex<HandleTables>>>,
    /// Every resource table of the component instance, by table
    /// index. A handle's declared type names the index; this maps it
    /// to the table the instance keeps and the resource it holds.
    resource_tables: Vec<Option<ResourceTableRuntime>>,
}

impl<'a, T: 'static> BoundaryContext<'a, T> {
    /// Build the context of one crossing between a host value and a
    /// guest, under `options`, for `instance`, counted against
    /// `scope`.
    pub fn new(
        store: StoreContextMut<'a, T, Backend>,
        options: BoundaryOptions,
        instance: Option<InstanceId>,
        scope: Option<Scope>,
        tables: Option<Arc<Mutex<HandleTables>>>,
        resource_tables: Vec<Option<ResourceTableRuntime>>,
    ) -> Self {
        let strategy = AbiStrategy::select(&options);
        Self {
            store,
            options,
            source: None,
            instance,
            scope,
            strategy,
            tables,
            resource_tables,
        }
    }

    /// Build the context of one copy between two guest memories,
    /// which is what an adapter's transcoder performs. The crossing
    /// reads through `source` and writes through `destination`, and
    /// carries no handles of its own.
    pub fn for_copy(
        store: StoreContextMut<'a, T, Backend>,
        destination: BoundaryOptions,
        source: BoundaryOptions,
        instance: Option<InstanceId>,
        scope: Option<Scope>,
    ) -> Self {
        let strategy = AbiStrategy::select(&destination);
        Self {
            store,
            options: destination,
            source: Some(source),
            instance,
            scope,
            strategy,
            tables: None,
            resource_tables: Vec::new(),
        }
    }

    /// The options of the crossing. Nothing reads them back yet: a
    /// `task.return` compares its own against the task's.
    #[allow(dead_code)]
    pub fn options(&self) -> &BoundaryOptions {
        &self.options
    }

    /// The component instance the crossing belongs to. Nothing reads
    /// it yet, for the reason the field states.
    #[allow(dead_code)]
    pub fn instance(&self) -> Option<InstanceId> {
        self.instance
    }

    /// The task or subtask the crossing counts against.
    pub fn scope(&self) -> Option<Scope> {
        self.scope
    }

    /// The strategy the crossing's options selected. The crossing
    /// itself goes through the strategy on the field; only a test
    /// asks the context which one it picked.
    #[allow(dead_code)]
    pub fn strategy(&self) -> AbiStrategy {
        self.strategy
    }

    /// The encoding a `string`-typed value crosses in.
    pub fn string_encoding(&self) -> StringEncoding {
        self.options.string_encoding()
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

    /// The canon options of the lift of the task the crossing
    /// counts against. A `task.return` must find its own equal to
    /// these; nothing else reads them, so nothing does yet.
    #[allow(dead_code)]
    pub fn task_lift_options(&self) -> Option<CanonOptions> {
        let Some(Scope::Task(task)) = self.scope else {
            return None;
        };
        let guard = self.tables.as_ref()?.lock().ok()?;
        guard.tasks.task(task)?.options.clone()
    }

    /// The result type of the function the task the crossing counts
    /// against is a call into. A `task.return` must find its own
    /// equal to this; nothing else reads it, so nothing does yet.
    #[allow(dead_code)]
    pub fn task_result_type(&self) -> Option<ValueType> {
        let Some(Scope::Task(task)) = self.scope else {
            return None;
        };
        let guard = self.tables.as_ref()?.lock().ok()?;
        let function: &FunctionType = guard.tasks.task(task)?.function.as_ref()?;
        function.result.clone()
    }

    /// The size of the guest's store of values in bytes, when the
    /// crossing's strategy addresses one of a bounded size.
    pub fn memory_size(&mut self) -> Option<usize> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy.size(store, options)
    }

    /// Whether `length` bytes at `offset` lie inside the guest's
    /// store of values. `true` when the crossing addresses no
    /// bounded store, so the read reports the absence itself.
    pub fn in_bounds(&mut self, offset: usize, length: usize) -> bool {
        match self.memory_size() {
            Some(size) => offset.checked_add(length).is_some_and(|end| end <= size),
            None => true,
        }
    }

    /// Read `length` bytes starting at `offset` out of the guest.
    /// Surfaces a structured [`AbiError`] labelled with `position`
    /// and `valtype`.
    pub fn read_bytes(
        &mut self,
        offset: usize,
        length: usize,
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<Vec<u8>> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy
            .load(store, options, offset, length)
            .map_err(|cause| Self::labelled(cause, position, valtype))
    }

    /// Write `bytes` at `offset` into the guest.
    pub fn write_bytes(
        &mut self,
        offset: usize,
        bytes: &[u8],
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<()> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy
            .store(store, options, offset, bytes)
            .map_err(|cause| Self::labelled(cause, position, valtype))
    }

    /// Ask the guest for `size` bytes at an explicit `alignment` and
    /// return the pointer it gave back. `valtype` and `position`
    /// only label the error when the request fails.
    pub fn allocate_aligned(
        &mut self,
        size: usize,
        alignment: usize,
        valtype: &ValueType,
        position: AbiPosition,
    ) -> Result<usize> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy
            .allocate(store, options, size, alignment)
            .map_err(|cause| Self::labelled(cause, position, valtype))
    }

    /// Read `length` bytes at `offset` out of the side a copy
    /// between two guest memories reads from. A crossing that is not
    /// such a copy reads its own side.
    pub fn read_source_bytes(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        let Self {
            store,
            options,
            source,
            strategy,
            ..
        } = self;
        strategy
            .load(store, source.as_ref().unwrap_or(options), offset, length)
            .map_err(Self::unlabelled)
    }

    /// Read `length` bytes at `offset` out of the side this crossing
    /// writes to, which a copy between two guest memories needs when
    /// it rewrites what it already copied.
    pub fn read_own_bytes(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy
            .load(store, options, offset, length)
            .map_err(Self::unlabelled)
    }

    /// Write `bytes` at `offset` into the side this crossing writes
    /// to, with no valtype to label a failure with.
    pub fn write_own_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        let Self {
            store,
            options,
            strategy,
            ..
        } = self;
        strategy
            .store(store, options, offset, bytes)
            .map_err(Self::unlabelled)
    }

    /// Run the export's `post-return` over the core results the
    /// export returned, once the caller has observed the return
    /// value. Does nothing when the options declare none.
    pub fn post_return(&mut self, core_results: &[RuntimeVal]) -> Result<()> {
        let Some(post_return) = self.options.post_return().cloned() else {
            return Ok(());
        };
        let mut empty: [RuntimeVal; 0] = [];
        post_return
            .call(&mut self.store, core_results, &mut empty)
            .map_err(|cause| {
                Error::from(AbiError {
                    position: AbiPosition::Result,
                    valtype: ValueType::Primitive(crate::types::PrimitiveType::Bool),
                    cause: AbiCause::SubstrateFailure(cause),
                })
            })
    }

    /// Label a strategy's cause with the slot and the value type the
    /// crossing was processing.
    fn labelled(cause: AbiCause, position: AbiPosition, valtype: &ValueType) -> Error {
        Error::from(AbiError {
            position,
            valtype: valtype.clone(),
            cause,
        })
    }

    /// Report a strategy's cause from a crossing that processes no
    /// value type: a copy between two guest memories moves code
    /// units, not a `Val`.
    fn unlabelled(cause: AbiCause) -> Error {
        Error::internal(format!("guest memory access failed: {cause}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::component::{FunctionParameter, FunctionType};
    use crate::engine::Engine;
    use crate::executor::ir::DataModel;
    use crate::store::Store;
    use crate::types::PrimitiveType;
    use wasm_runtime_layer::AsContextMut;

    /// Canon options that name no runtime slot, under `data_model`.
    fn canon(data_model: DataModel) -> CanonOptions {
        CanonOptions {
            instance: 0,
            memory: None,
            realloc: None,
            post_return: None,
            string_encoding: StringEncoding::Utf8,
            data_model,
        }
    }

    /// An instance runtime state holding one component instance and
    /// no filled slot.
    fn state(instance: InstanceId) -> Arc<Mutex<AbiRuntimeState>> {
        Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
            0,
            0,
            0,
            Vec::new(),
            vec![instance],
        )))
    }

    /// The `(result u32)` signature a task is a call into.
    fn signature() -> FunctionType {
        FunctionType {
            parameters: vec![FunctionParameter {
                name: "x".to_owned(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::U32)),
        }
    }

    #[test]
    fn it_builds_one_context_from_options_an_instance_and_a_scope() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let tables = store.tables_handle();
        let (instance, task) = {
            let mut guard = tables.lock().expect("tables");
            let instance = guard.tasks.insert_instance();
            let declared = canon(DataModel::LinearMemory);
            let task = guard
                .tasks
                .push_task(Some(signature()), Some(declared), instance);
            (instance, task)
        };
        let declared = canon(DataModel::LinearMemory);
        let options =
            BoundaryOptions::resolve(&declared, &state(instance)).expect("options resolve");
        let ctx = BoundaryContext::new(
            store.inner_mut().as_context_mut(),
            options,
            Some(instance),
            Some(Scope::Task(task)),
            Some(tables.clone()),
            Vec::new(),
        );

        assert_eq!(
            ctx.options().declared(),
            Some(&declared),
            "the crossing carries its canon options as a value"
        );
        assert_eq!(
            ctx.instance(),
            Some(instance),
            "the crossing names the component instance it belongs to"
        );
        assert_eq!(
            ctx.scope(),
            Some(Scope::Task(task)),
            "the crossing names the task its borrows count against"
        );
        assert_eq!(
            ctx.task_lift_options().as_ref(),
            Some(&declared),
            "the lift options of the task are on the context"
        );
        assert_eq!(
            ctx.task_result_type(),
            Some(ValueType::Primitive(PrimitiveType::U32)),
            "so is the result type of the task's function"
        );
    }

    #[test]
    fn it_selects_the_eager_strategy_from_the_linear_memory_data_model() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = InstanceId::from_index(0);
        let options = BoundaryOptions::resolve(&canon(DataModel::LinearMemory), &state(instance))
            .expect("options resolve");
        let mut ctx = BoundaryContext::new(
            store.inner_mut().as_context_mut(),
            options,
            Some(instance),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(ctx.strategy(), AbiStrategy::Eager);
        // The eager strategy addresses linear memory, so a crossing
        // whose options name none reports the absent memory rather
        // than the absent strategy.
        let ty = ValueType::Primitive(crate::types::PrimitiveType::U32);
        let Err(Error::Abi(error)) = ctx.read_bytes(0, 4, AbiPosition::Result, &ty) else {
            panic!("the read reports the memory the options do not name");
        };
        assert!(matches!(
            error.cause,
            AbiCause::OutOfBoundsMemory {
                offset: 0,
                length: 4
            }
        ));
    }

    #[test]
    fn it_selects_a_second_strategy_from_another_data_model() {
        // The call site is the same `BoundaryContext::new` every
        // crossing uses: only the data model of the options differs,
        // and the context selects the other strategy from it.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = InstanceId::from_index(0);
        let options = BoundaryOptions::resolve(&canon(DataModel::Gc), &state(instance))
            .expect("options resolve");
        let mut ctx = BoundaryContext::new(
            store.inner_mut().as_context_mut(),
            options,
            Some(instance),
            None,
            None,
            Vec::new(),
        );

        assert_eq!(
            ctx.strategy(),
            AbiStrategy::Lazy,
            "the second strategy sits beside the eager one behind the same context"
        );
        let ty = ValueType::Primitive(crate::types::PrimitiveType::U32);
        for outcome in [
            ctx.read_bytes(0, 4, AbiPosition::Result, &ty).map(|_| ()),
            ctx.write_bytes(0, &[0u8; 4], AbiPosition::Result, &ty),
            ctx.allocate_aligned(4, 4, &ty, AbiPosition::Result)
                .map(|_| ()),
        ] {
            let Err(Error::Abi(error)) = outcome else {
                panic!("the polyfill implements no access under the second strategy");
            };
            assert!(matches!(error.cause, AbiCause::UnsupportedDataModel));
        }
        assert_eq!(
            ctx.memory_size(),
            None,
            "the second strategy addresses no linear memory"
        );
    }
}
