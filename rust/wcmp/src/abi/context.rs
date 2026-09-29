//! The boundary context: the one lift and lower crossing.
//!
//! A boundary context is the object through which one value crosses
//! between the host's [`Val`] and the guest's memory or flat slots.
//! One context is built per crossing, from three things: the canon
//! options of the lift or lower, the component instance, and the
//! task or subtask whose borrows and lends the crossing counts
//! against. The tables a handle of the crossing resolves against
//! are the instance's, and the context reads them off it, so a call
//! site names no table of its own.
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
//! Every context carries a copy budget: the bytes of host values the
//! crossing may still build out of the guest. It starts at the
//! store's hostcall fuel, 128 MiB unless the host set another amount,
//! and each list, string, and map the crossing lifts spends from it.
//! One call into the host, one `task.return`, and one result a host
//! reads back each build a context of their own, so each has one
//! budget.
//!
//! Construction is workspace-internal — the context is always built
//! immediately before a call drives the canonical ABI.
//!
//! [`Val`]: crate::value::Val

use std::sync::Arc;

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::instance::BoundaryInstance;
use crate::abi::options::BoundaryOptions;
use crate::abi::signature::Signature;
use crate::abi::strategy::AbiStrategy;
use crate::concurrency::Scope;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::{CanonOptions, StringEncoding};
use crate::internal::ErrorInternal;
use crate::runtime_layer::{Backend, StoreContextMut, Val as RuntimeVal};
use crate::store::StoreData;
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
    /// reads from, with the strategy those options select. `None`
    /// for every crossing that is not such a copy, which is every
    /// crossing between a host [`Val`] and a guest: there the side
    /// read from is the side written to.
    ///
    /// [`Val`]: crate::value::Val
    source: Option<(BoundaryOptions, AbiStrategy)>,
    /// The component instance the crossing belongs to, and with it
    /// the tables a handle of the crossing resolves against. A lift
    /// or a lower takes the instance from its options. A copy
    /// between two guest memories takes it from the task on the
    /// stack, because the fused adapter's transcoder names only the
    /// two memories, and has none when nothing is on the stack.
    instance: BoundaryInstance,
    /// The task or subtask the crossing's borrows and lends count
    /// against. Absent only for a crossing driven with no call in
    /// flight, which no guest reaches.
    scope: Option<Scope>,
    /// The strategy the options selected.
    strategy: AbiStrategy,
    /// The guest bytes of the list whose elements the crossing is
    /// lifting, read in one access. A read that falls inside them is
    /// served from here rather than from the guest.
    window: Option<GuestBytes>,
    /// The guest bytes of the list whose elements the crossing is
    /// lowering, assembled here and written to the guest in one
    /// access once the last element is in. A write that falls inside
    /// them lands here rather than in the guest.
    staged: Option<GuestBytes>,
    /// How many reads and writes the crossing has made of the guest
    /// itself, which is the cost a browser pays a JavaScript boundary
    /// crossing apiece for.
    accesses: usize,
    /// How many more bytes of host values the crossing may build out
    /// of the guest.
    budget_left: usize,
    /// Whether the values of the crossing come from one guest and go
    /// to another, as they do in a copy through a stream or a future
    /// whose two ends two components hold. An error context lifted
    /// anywhere else goes to the host, and its lift marks the record
    /// host-held.
    between_guests: bool,
}

/// A range of guest bytes the crossing holds on the host side: the
/// offset of the first, and the bytes.
struct GuestBytes {
    offset: usize,
    bytes: Vec<u8>,
}

impl GuestBytes {
    /// The `length` bytes at `offset`, when all of them fall inside
    /// the range.
    fn slice(&self, offset: usize, length: usize) -> Option<&[u8]> {
        let start = offset.checked_sub(self.offset)?;
        self.bytes.get(start..start.checked_add(length)?)
    }

    /// The `length` bytes at `offset`, writable, when all of them fall
    /// inside the range.
    fn slice_mut(&mut self, offset: usize, length: usize) -> Option<&mut [u8]> {
        let start = offset.checked_sub(self.offset)?;
        self.bytes.get_mut(start..start.checked_add(length)?)
    }
}

impl<'a, T: 'static> BoundaryContext<'a, StoreData<T>> {
    /// Build the context of one crossing between a host value and a
    /// guest, under `options`, for `instance`, counted against
    /// `scope`. The tables the crossing resolves a handle against
    /// come off the instance, and the copy budget off the store.
    pub fn new(
        store: StoreContextMut<'a, StoreData<T>, Backend>,
        options: BoundaryOptions,
        instance: BoundaryInstance,
        scope: Option<Scope>,
    ) -> Self {
        let strategy = AbiStrategy::select(&options);
        let budget_left = store.data().hostcall_fuel();
        Self {
            store,
            options,
            source: None,
            instance,
            scope,
            strategy,
            window: None,
            staged: None,
            accesses: 0,
            budget_left,
            between_guests: false,
        }
    }

    /// Build the context of one copy between two guest memories,
    /// which is what an adapter's transcoder performs. The crossing
    /// reads through `source` and writes through `destination`, and
    /// carries no handles of its own.
    pub fn for_copy(
        store: StoreContextMut<'a, StoreData<T>, Backend>,
        destination: BoundaryOptions,
        source: BoundaryOptions,
        instance: BoundaryInstance,
        scope: Option<Scope>,
    ) -> Self {
        let strategy = AbiStrategy::select(&destination);
        let source_strategy = AbiStrategy::select(&source);
        let budget_left = store.data().hostcall_fuel();
        Self {
            store,
            options: destination,
            source: Some((source, source_strategy)),
            instance,
            scope,
            strategy,
            window: None,
            staged: None,
            accesses: 0,
            budget_left,
            between_guests: false,
        }
    }
}

impl<'a, T: 'static> BoundaryContext<'a, T> {
    /// The options of the crossing. Nothing reads them back yet: a
    /// `task.return` compares its own against the task's.
    #[allow(dead_code)]
    pub fn options(&self) -> &BoundaryOptions {
        &self.options
    }

    /// The component instance the crossing belongs to, through which
    /// a handle of the crossing reaches the tables it resolves
    /// against.
    pub fn instance(&self) -> &BoundaryInstance {
        &self.instance
    }

    /// The task or subtask the crossing counts against.
    pub fn scope(&self) -> Option<Scope> {
        self.scope
    }

    /// Mark the crossing as one side of a move of values from one
    /// guest to another: a lift whose values another guest lowers,
    /// or a lower of values another guest lifted.
    pub fn between_guests(mut self) -> Self {
        self.between_guests = true;
        self
    }

    /// Whether the crossing is one side of a move of values from one
    /// guest to another.
    pub fn crosses_between_guests(&self) -> bool {
        self.between_guests
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

    /// The canon options of the lift of the task the crossing
    /// counts against. A `task.return` must find its own equal to
    /// these, and reads the task's `async` option off them.
    pub fn task_lift_options(&self) -> Option<Arc<CanonOptions>> {
        let Some(Scope::Task(task)) = self.scope else {
            return None;
        };
        let guard = self.instance.tables()?.lock().ok()?;
        guard.tasks.task(task)?.options.clone()
    }

    /// The result type of the function the task the crossing counts
    /// against is a call into. A `task.return` must find its own
    /// equal to this.
    pub fn task_result_type(&self) -> Option<ValueType> {
        let Some(Scope::Task(task)) = self.scope else {
            return None;
        };
        let guard = self.instance.tables()?.lock().ok()?;
        let function: &Signature = guard.tasks.task(task)?.function.as_deref()?;
        function.ty().result.clone()
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
    /// and `valtype`. Bytes that fall inside the list the crossing is
    /// lifting come out of the copy it already read.
    pub fn read_bytes(
        &mut self,
        offset: usize,
        length: usize,
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<Vec<u8>> {
        if let Some(bytes) = self.window.as_ref().and_then(|w| w.slice(offset, length)) {
            return Ok(bytes.to_vec());
        }
        self.load(offset, length)
            .map_err(|cause| Self::labelled(cause, position, valtype))
    }

    /// Read the `N` bytes starting at `offset` out of the guest, as
    /// [`read_bytes`](Self::read_bytes) does, into an array rather
    /// than a fresh allocation: a scalar is read this way.
    pub fn read_array<const N: usize>(
        &mut self,
        offset: usize,
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        match self.window.as_ref().and_then(|w| w.slice(offset, N)) {
            Some(bytes) => out.copy_from_slice(bytes),
            None => out.copy_from_slice(&self.read_bytes(offset, N, position, valtype)?),
        }
        Ok(out)
    }

    /// Read the `length` bytes at `offset` in one access and serve
    /// every read `lift` makes inside them from that copy until `lift`
    /// returns. A list's elements are lifted this way, so the list
    /// costs one access of the guest however many elements and fields
    /// it has; a read outside the range — the bytes of a string an
    /// element points to — still goes to the guest. Nothing runs
    /// guest code during a lift, so the copy cannot go stale.
    pub fn lifting_within<R>(
        &mut self,
        offset: usize,
        length: usize,
        position: AbiPosition,
        valtype: &ValueType,
        lift: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        let bytes = if length == 0 {
            Vec::new()
        } else {
            self.read_bytes(offset, length, position, valtype)?
        };
        let outer = self.window.replace(GuestBytes { offset, bytes });
        let lifted = lift(self);
        self.window = outer;
        lifted
    }

    /// Write `bytes` at `offset` into the guest. Bytes that fall
    /// inside the list the crossing is lowering land in the copy it
    /// is assembling.
    pub fn write_bytes(
        &mut self,
        offset: usize,
        bytes: &[u8],
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<()> {
        if let Some(target) = self
            .staged
            .as_mut()
            .and_then(|s| s.slice_mut(offset, bytes.len()))
        {
            target.copy_from_slice(bytes);
            return Ok(());
        }
        self.store_bytes(offset, bytes)
            .map_err(|cause| Self::labelled(cause, position, valtype))
    }

    /// Assemble the `length` bytes at `offset` on the host while
    /// `lower` writes into them, then write them to the guest in one
    /// access. A list's elements are lowered this way, so the list
    /// costs one access of the guest however many elements and fields
    /// it has. Bytes `lower` leaves unwritten — padding, and the tail
    /// of a variant's payload area its case does not use — reach the
    /// guest as zeros; the canonical ABI gives them no value.
    ///
    /// A write outside the range goes to the guest as `lower` makes
    /// it. The range is the allocation `cabi_realloc` returned for
    /// the list, and a `cabi_realloc` the elements call for strings
    /// and nested lists hands out memory outside it, so the order in
    /// which the list's own bytes reach the guest is not observable.
    pub fn lowering_within(
        &mut self,
        offset: usize,
        length: usize,
        position: AbiPosition,
        valtype: &ValueType,
        lower: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        let outer = self.staged.replace(GuestBytes {
            offset,
            bytes: vec![0u8; length],
        });
        let lowered = lower(self);
        let staged = std::mem::replace(&mut self.staged, outer);
        lowered?;
        match staged {
            Some(staged) if !staged.bytes.is_empty() => {
                self.write_bytes(staged.offset, &staged.bytes, position, valtype)
            }
            _ => Ok(()),
        }
    }

    /// Charge `count` host values of `size` bytes apiece against the
    /// crossing's copy budget, or refuse them with
    /// [`AbiCause::CopyBudgetSpent`] when they cost more than the
    /// budget has left. A lift charges before it reserves anything
    /// for the values, and a refused charge spends nothing.
    pub fn charge_copy_budget(
        &mut self,
        count: usize,
        size: usize,
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<()> {
        match count
            .checked_mul(size)
            .and_then(|cost| self.budget_left.checked_sub(cost))
        {
            Some(left) => {
                self.budget_left = left;
                Ok(())
            }
            None => Err(Error::from(AbiError {
                position,
                valtype: Some(valtype.clone()),
                cause: AbiCause::CopyBudgetSpent,
            })),
        }
    }

    /// How many reads and writes the crossing has made of the guest
    /// itself, as opposed to those served on the host side.
    #[cfg(test)]
    pub fn substrate_accesses(&self) -> usize {
        self.accesses
    }

    /// Load `length` bytes at `offset` through the crossing's own
    /// strategy, and count the access.
    fn load(&mut self, offset: usize, length: usize) -> std::result::Result<Vec<u8>, AbiCause> {
        let Self {
            store,
            options,
            strategy,
            accesses,
            ..
        } = self;
        *accesses += 1;
        strategy.load(store, options, offset, length)
    }

    /// Store `bytes` at `offset` through the crossing's own strategy,
    /// and count the access.
    fn store_bytes(&mut self, offset: usize, bytes: &[u8]) -> std::result::Result<(), AbiCause> {
        let Self {
            store,
            options,
            strategy,
            accesses,
            ..
        } = self;
        *accesses += 1;
        strategy.store(store, options, offset, bytes)
    }

    /// Ask the guest for `size` bytes at an explicit `alignment` and
    /// return the pointer it gave back. `valtype` and `position`
    /// only label the error when the request fails.
    ///
    /// The request is a call into the guest's `cabi_realloc`, so it
    /// runs as a [`BoundaryCall`]: on a task of its own, with one
    /// fresh thread, and with the instance's may-leave flag clear.
    /// The call ends whether the realloc returned or failed.
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
            instance,
            ..
        } = self;
        let call = BoundaryCall::realloc(instance, &mut *store)?;
        let allocated = strategy
            .allocate(store, options, size, alignment)
            .map_err(|cause| Self::labelled(cause, position, valtype));
        let ended = call.end(&mut *store);
        allocated.and_then(|pointer| ended.map(|()| pointer))
    }

    /// Read `length` bytes at `offset` out of the side a copy
    /// between two guest memories reads from, under the strategy
    /// that side's own options select: each side of a copy carries
    /// its own data model, so the side read from decides how the
    /// read happens. A crossing that is not such a copy reads its
    /// own side, under its own strategy.
    pub fn read_source_bytes(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        let Self {
            store,
            options,
            source,
            strategy,
            accesses,
            ..
        } = self;
        *accesses += 1;
        let (options, strategy) = match source {
            Some((source_options, source_strategy)) => (&*source_options, &*source_strategy),
            None => (&*options, &*strategy),
        };
        strategy
            .load(store, options, offset, length)
            .map_err(Self::unlabelled)
    }

    /// Read `length` bytes at `offset` out of the side this crossing
    /// writes to, which a copy between two guest memories needs when
    /// it rewrites what it already copied.
    pub fn read_own_bytes(&mut self, offset: usize, length: usize) -> Result<Vec<u8>> {
        self.load(offset, length).map_err(Self::unlabelled)
    }

    /// Write `bytes` at `offset` into the side this crossing writes
    /// to, with no valtype to label a failure with.
    pub fn write_own_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        self.store_bytes(offset, bytes).map_err(Self::unlabelled)
    }

    /// Run the export's `post-return` over the core results the
    /// export returned, once the caller has observed the return
    /// value. Does nothing when the options declare none.
    ///
    /// The `post-return` is a call into the guest, so it runs as a
    /// [`BoundaryCall`] with the instance's may-leave flag clear. It
    /// runs inside the export's own task, as the reference calls it,
    /// so it takes no task of its own.
    pub fn post_return(&mut self, core_results: &[RuntimeVal]) -> Result<()> {
        let Some(post_return) = self.options.post_return().cloned() else {
            return Ok(());
        };
        let call = BoundaryCall::post_return(&self.instance, &mut self.store)?;
        let mut empty: [RuntimeVal; 0] = [];
        let ran = post_return
            .call(&mut self.store, core_results, &mut empty)
            .map_err(|cause| {
                Error::from(AbiError {
                    position: AbiPosition::Result,
                    valtype: None,
                    cause: AbiCause::SubstrateFailure(cause),
                })
            });
        let ended = call.end(&mut self.store);
        ran.and(ended)
    }

    /// Label a strategy's cause with the slot and the value type the
    /// crossing was processing.
    fn labelled(cause: AbiCause, position: AbiPosition, valtype: &ValueType) -> Error {
        Error::from(AbiError {
            position,
            valtype: Some(valtype.clone()),
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::abi::instance_flags::InstanceFlags;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::component::{FunctionParameter, FunctionType};
    use crate::concurrency::InstanceId;
    use crate::engine::Engine;
    use crate::executor::ir::DataModel;
    use crate::resource::TableId;
    use crate::runtime_layer::AsContextMut;
    use crate::store::Store;
    use crate::store::StoreInternalExt;
    use crate::types::PrimitiveType;

    /// Canon options that name no runtime slot, under `data_model`.
    fn canon(data_model: DataModel) -> Arc<CanonOptions> {
        Arc::new(CanonOptions {
            instance: 0,
            memory: None,
            realloc: None,
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model,
        })
    }

    /// An instance runtime state holding one component instance and
    /// no filled slot. `flags` carries the instance's may-leave flag
    /// for a crossing that calls the guest's `cabi_realloc`, which
    /// clears it, and is empty for one that does not.
    fn state(instance: InstanceId, flags: Vec<InstanceFlags>) -> Arc<Mutex<AbiRuntimeState>> {
        Arc::new(Mutex::new(
            AbiRuntimeState::with_slabs(
                0,
                0,
                0,
                0,
                Vec::new(),
                vec![instance],
                vec![TableId::fresh()],
            )
            .with_instance_flags(flags),
        ))
    }

    /// The `(result u32)` signature a task is a call into.
    fn signature() -> Arc<Signature> {
        Arc::new(Signature::new(FunctionType {
            parameters: vec![FunctionParameter {
                name: "x".to_owned(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            }],
            result: Some(ValueType::Primitive(PrimitiveType::U32)),
            async_: false,
        }))
    }

    #[wcmp_macros::test]
    fn it_builds_one_context_from_options_an_instance_and_a_scope() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let tables = store.internal().tables_handle();
        let (instance, task) = {
            let mut guard = tables.lock().expect("tables");
            let instance = guard.tasks.insert_instance();
            let declared = canon(DataModel::LinearMemory);
            let task = guard
                .tasks
                .push_task(Some(signature()), Some(declared), instance)
                .expect("room under the record cap");
            (instance, task)
        };
        let declared = canon(DataModel::LinearMemory);
        // The three inputs of a crossing: the options, the instance
        // the options name — which carries the tables of the
        // crossing — and the scope.
        let (options, boundary_instance) =
            BoundaryInstance::resolve(&declared, &state(instance, Vec::new()), &tables)
                .expect("resolve");
        let ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            boundary_instance,
            Some(Scope::Task(task)),
        );

        assert_eq!(
            ctx.options().declared(),
            Some(&*declared),
            "the crossing carries its canon options as a value"
        );
        assert_eq!(
            ctx.instance().id(),
            Some(instance),
            "the crossing names the component instance it belongs to"
        );
        assert!(
            ctx.instance().tables().is_some(),
            "and the tables of the crossing came off that instance"
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

    #[wcmp_macros::test]
    fn it_selects_the_eager_strategy_from_the_linear_memory_data_model() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = InstanceId::from_index(0);
        let tables = store.internal().tables_handle();
        let (options, instance) = BoundaryInstance::resolve(
            &canon(DataModel::LinearMemory),
            &state(instance, Vec::new()),
            &tables,
        )
        .expect("resolve");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
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

    #[wcmp_macros::test]
    fn it_selects_a_second_strategy_from_another_data_model() {
        // The call site is the same `BoundaryContext::new` every
        // crossing uses: only the data model of the options differs,
        // and the context selects the other strategy from it.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let tables = store.internal().tables_handle();
        // The allocation below enters a `cabi_realloc` boundary
        // call, which clears the may-leave flag of the instance the
        // options name. The store therefore has to hold that
        // instance's record, so the identity is minted from the
        // records rather than made up.
        let instance = tables
            .lock()
            .expect("handle tables")
            .tasks
            .insert_instance();
        let flags = InstanceFlags::new(store.internal().inner_mut().as_context_mut());
        let (options, instance) = BoundaryInstance::resolve(
            &canon(DataModel::Gc),
            &state(instance, vec![flags]),
            &tables,
        )
        .expect("resolve");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
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

    #[wcmp_macros::test]
    fn it_reads_the_source_of_a_copy_under_the_source_strategy() {
        // A copy between two guest memories has two sides, and each
        // carries its own data model. The read of the source goes
        // through the strategy the source's options select, not the
        // destination's: here the source is under the second
        // strategy, which implements no access, while the
        // destination is under the eager one.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = InstanceId::from_index(0);
        let state = state(instance, Vec::new());
        let (destination, source) = {
            let guard = state.lock().expect("runtime state");
            (
                BoundaryOptions::from_state(&canon(DataModel::LinearMemory), &guard),
                BoundaryOptions::from_state(&canon(DataModel::Gc), &guard),
            )
        };
        let mut ctx = BoundaryContext::for_copy(
            store.internal().inner_mut().as_context_mut(),
            destination,
            source,
            BoundaryInstance::without_tables(Some(instance)),
            None,
        );

        let Err(Error::Internal { message }) = ctx.read_source_bytes(0, 4) else {
            panic!("the source read reports the strategy of the side it reads");
        };
        assert!(
            message.contains("data model is not implemented"),
            "the source's own strategy refused the read, not the destination's: {message}"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_charge_past_the_copy_budget_and_spends_nothing_on_it() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        assert_eq!(store.hostcall_fuel(), 128 << 20, "Wasmtime's default");
        // The context starts from the fuel the store holds when it is
        // built.
        store.set_hostcall_fuel(40);
        let instance = InstanceId::from_index(0);
        let tables = store.internal().tables_handle();
        let (options, instance) = BoundaryInstance::resolve(
            &canon(DataModel::LinearMemory),
            &state(instance, Vec::new()),
            &tables,
        )
        .expect("resolve");
        let mut ctx = BoundaryContext::new(
            store.internal().inner_mut().as_context_mut(),
            options,
            instance,
            None,
        );
        let ty = ValueType::Primitive(PrimitiveType::String);

        ctx.charge_copy_budget(32, 1, AbiPosition::Result, &ty)
            .expect("a charge inside the budget is taken");
        for (count, size) in [(9, 1), (3, 3), (usize::MAX, 2)] {
            let Err(Error::Abi(error)) =
                ctx.charge_copy_budget(count, size, AbiPosition::Result, &ty)
            else {
                panic!("{count} values of {size} bytes cost more than the 8 bytes left");
            };
            assert!(matches!(error.cause, AbiCause::CopyBudgetSpent));
        }
        ctx.charge_copy_budget(4, 2, AbiPosition::Result, &ty)
            .expect("the refused charges spent nothing, so the last 8 bytes are left");
        assert!(
            ctx.charge_copy_budget(1, 1, AbiPosition::Result, &ty)
                .is_err(),
            "and then nothing is"
        );
    }

    #[wcmp_macros::test]
    fn it_labels_a_realloc_failure_with_the_slot_and_the_value_type() {
        let cause = AbiCause::ReallocFailed(anyhow::anyhow!("cabi_realloc trapped"));
        let valtype = ValueType::Primitive(PrimitiveType::U32);

        let error = BoundaryContext::<()>::labelled(cause, AbiPosition::Argument(0), &valtype);

        let Error::Abi(abi) = error else {
            panic!("a failure of the realloc is an ABI failure");
        };
        assert!(
            matches!(abi.position, AbiPosition::Argument(0)),
            "the failure names the slot the crossing was processing"
        );
        assert_eq!(
            abi.valtype,
            Some(ValueType::Primitive(PrimitiveType::U32)),
            "and the value type it was processing"
        );
    }
}
