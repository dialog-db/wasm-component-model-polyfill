//! The `task.return` built-in.
//!
//! A `canon task.return` definition names a result type and the
//! canon options the result is lifted under, and the guest imports
//! it as a core function whose parameters are the flattened result.
//! An export lifted `canon lift async` does not return its result by
//! returning from its core function; it calls this built-in, which
//! resolves the task the call is.
//!
//! The built-in resolves the task on top of the store's stack of
//! current scopes and traps in the order the reference gives:
//!
//! 1. The instance of that task must be leavable. A `cabi_realloc`
//!    and an export's `post-return` both clear the flag, so neither
//!    can return a result.
//! 2. The task's lift must carry the `async` option. A synchronous
//!    export returns its result by returning, so the built-in is not
//!    for it. The reference traps on the same condition and Wasmtime
//!    has no trap of its own for it.
//! 3. The built-in's result type must equal the result of the
//!    function the task is a call into, structurally; its string
//!    encoding must equal the one the task's lift declared; and its
//!    memory must be the task's memory. A built-in whose options
//!    name no memory passes that last comparison, because validation
//!    lets it leave the memory out only when the result needs none.
//!
//! The result is then lifted through one boundary context built from
//! the built-in's options, the instance, and the task, so a string or
//! a list result reads the task's memory. Two further traps follow
//! the lift, as the reference's `Task.return_` applies them: a task
//! that has already resolved cannot resolve again, and a task that
//! still owes a borrow cannot return, which is the scope-exit rule
//! every other end of a call applies.
//!
//! A task the prepare intrinsic of a fused adapter created takes a
//! different crossing. The adapter generated a return function for
//! the call, and running that function is the crossing: it lowers
//! the callee's result straight into the caller's memory, and the
//! built-in lifts no value of the host's at all. The reference calls
//! that moment `on_resolve`. Such a task also compares its result
//! type by the interned index of the tuple the adapter named, since
//! the adapter names the type at run time and the polyfill has no
//! projection of it; that is the comparison Wasmtime makes for the
//! same call. The two traps that follow the lift come before the
//! crossing there, because the return function must not run twice.

use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};

use crate::abi::context::BoundaryContext;
use crate::abi::flatten::lift_from_flat_slots;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{FlatType, MAX_FLAT_PARAMS, flat_count};
use crate::abi::lift;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::backend::Backend;
use crate::concurrency::{InstanceId, Scope, SubtaskId, TaskId, TaskState};
use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result, ReturnMismatchKind,
    TaskCause,
};
use crate::executor::intrinsics::core_func_type;
use crate::executor::ir::{CanonOptions, CoreSignature};
use crate::resource::HandleTables;
use crate::store::{StoreContext, StoreData};
use crate::types::ValueType;
use crate::value::Val;

/// Build the `task.return` built-in one `canon task.return`
/// definition declares: `result` is the type it was declared with,
/// `options` the canon options it lifts under, and `signature` the
/// core signature the guest imports it at.
pub fn build_task_return<T: 'static>(
    store: &mut StoreContext<'_, T>,
    result: Option<ValueType>,
    result_tuple: usize,
    options: &CanonOptions,
    signature: &CoreSignature,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let declared = options.clone();
    RuntimeFunc::new(
        store.runtime_mut(),
        core_func_type(signature),
        move |store_ctx, args, _results| {
            Ok(task_return(
                store_ctx,
                &declared,
                result.as_ref(),
                result_tuple,
                &abi_state,
                &tables,
                args,
            )?)
        },
    )
}

/// Resolve the current task with the result `args` carries, which is
/// what one call of the built-in does. The module documentation
/// states the order of the traps.
fn task_return<T: 'static>(
    mut store_ctx: RuntimeContextMut<'_, StoreData<T>, Backend>,
    declared: &CanonOptions,
    result: Option<&ValueType>,
    result_tuple: usize,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
) -> Result<()> {
    let current = current_task(tables)?;
    let (task, task_instance) = (current.task, current.instance);
    if !current.may_leave {
        return Err(Error::Task(TaskCause::CannotLeave));
    }

    // The one crossing of the built-in: the result is lifted under
    // the built-in's own options, into the instance the task belongs
    // to, and counted against the task being resolved. The task is
    // also where the comparisons below read the lift's options and
    // result type from.
    let (options, instance) = BoundaryInstance::resolve(declared, abi_state, tables)?;
    let instance = instance.with_id(task_instance);
    let mut context = BoundaryContext::new(
        store_ctx.as_context_mut(),
        options,
        instance,
        Some(Scope::Task(task)),
    );

    let lift_options = match context.task_lift_options() {
        Some(options) if options.async_ => options,
        _ => return Err(Error::Task(TaskCause::ReturnFromSynchronousTask)),
    };
    // The result type is compared structurally for a task whose
    // function type the polyfill projected, and by the interned
    // index of the result tuple for a task the prepare intrinsic
    // created. An adapter names that type by index at run time and
    // there is nothing to project it against, so the index is what
    // travels; it is also the comparison Wasmtime makes for the same
    // call.
    let result_matches = match current.result_tuple {
        Some(prepared) => prepared == result_tuple,
        None => result.cloned() == context.task_result_type(),
    };
    if let Some(kind) = mismatch(result_matches, declared, &lift_options) {
        return Err(Error::Task(TaskCause::ReturnMismatch { kind }));
    }

    // A task the prepare intrinsic created does not lift its result
    // into a value of the host's. The adapter generated a return
    // function for the call, and running it is the crossing: it
    // lowers the callee's result straight into the caller's memory.
    if let Some(subtask) = current.subtask {
        drop(context);
        return cross_through_return_function(&mut store_ctx, tables, task, subtask, result, args);
    }

    let value = match result {
        None => None,
        Some(ty) if spills(ty) => {
            let pointer = pointer_argument(args)?;
            Some(lift(&mut context, pointer, ty, AbiPosition::Result)?)
        }
        Some(ty) => {
            let mut cursor = 0usize;
            Some(lift_from_flat_slots(
                &mut context,
                args,
                &mut cursor,
                ty,
                AbiPosition::Result,
            )?)
        }
    };
    drop(context);

    resolve(tables, task, value, result)
}

/// What the built-in reads off the task it is about to resolve.
struct Current {
    /// The task itself.
    task: TaskId,
    /// The component instance the task belongs to.
    instance: InstanceId,
    /// Whether that instance may be left.
    may_leave: bool,
    /// The subtask of the call the task is the callee of, for a call
    /// between two components the prepare intrinsic set up. `None`
    /// for a call from the host.
    subtask: Option<SubtaskId>,
    /// The interned result tuple the adapter named for such a call.
    result_tuple: Option<usize>,
}

/// The task the built-in resolves, with what the traps below
/// consult.
///
/// The task is the innermost one on the store's stack of current
/// scopes, which is the reference's current task. For a
/// `cabi_realloc` the polyfill called it is the realloc's own task,
/// whose instance may not be left, which is how the built-in refuses
/// a realloc that calls it. The one task that belongs to no
/// component instance, the destructor task of a resource the host
/// implements, runs host code only, so guest code never reaches the
/// built-in from it; that case is an internal error, not a trap.
fn current_task(tables: &Arc<Mutex<HandleTables>>) -> Result<Current> {
    let guard = lock(tables)?;
    let task = guard
        .tasks
        .current_task()
        .ok_or_else(|| Error::internal("`task.return` ran with no task on the stack"))?;
    let record = guard
        .tasks
        .task(task)
        .ok_or_else(|| Error::internal("the current task has no record"))?;
    let (subtask, result_tuple) = (record.subtask, record.result_tuple);
    let instance = record
        .instance
        .ok_or_else(|| Error::internal("the current task belongs to no component instance"))?;
    let may_leave = guard
        .tasks
        .instance(instance)
        .map(|record| record.may_leave)
        .ok_or_else(|| Error::internal("the current task names no instance record"))?;
    Ok(Current {
        task,
        instance,
        may_leave,
        subtask,
        result_tuple,
    })
}

/// Cross the callee's result into the caller by running the return
/// function the fused adapter generated for the call.
///
/// This is the reference's `on_resolve`, and it runs where the
/// reference runs it: at the callee's `task.return`, before the
/// callee's callback continues. The function takes the built-in's
/// own arguments, with the caller's return pointer appended when the
/// caller takes its result through one, and gives back the caller's
/// flat results, which the start intrinsic hands to the caller as it
/// returns.
///
/// The function is the caller's code, so it runs with the caller's
/// task as the current scope. The callee's scope goes back on the
/// stack afterwards, whichever way the crossing went, because the
/// callee's core function or callback is still below this frame.
fn cross_through_return_function<T: 'static>(
    store_ctx: &mut RuntimeContextMut<'_, StoreData<T>, Backend>,
    tables: &Arc<Mutex<HandleTables>>,
    task: TaskId,
    subtask: SubtaskId,
    result: Option<&ValueType>,
    args: &[RuntimeVal],
) -> Result<()> {
    // The two traps of a resolution come before the crossing here,
    // where the reference and Wasmtime both put them: a second
    // `task.return` must not run the return function twice, and a
    // task that still owes a borrow must not hand its result on.
    let (return_, arguments, mut results, caller) = {
        let guard = lock(tables)?;
        let record = guard
            .tasks
            .task(task)
            .ok_or_else(|| Error::internal("the current task has no record"))?;
        if record.state == TaskState::Resolved {
            return Err(Error::Task(TaskCause::ReturnedTwice));
        }
        if record.num_borrows > 0 {
            return Err(outstanding_borrows(record.num_borrows, result));
        }
        let bridge = guard
            .tasks
            .subtask(subtask)
            .and_then(|record| record.bridge.as_ref())
            .ok_or_else(|| Error::internal("a prepared call carries no generated functions"))?;
        let mut arguments = args.to_vec();
        if bridge.caller.has_return_pointer() {
            arguments.push(
                bridge
                    .arguments
                    .last()
                    .cloned()
                    .ok_or_else(|| Error::internal("a prepared call passed no return pointer"))?,
            );
        }
        let results: Vec<RuntimeVal> = bridge.caller_results.iter().copied().map(zero).collect();
        let caller = guard
            .tasks
            .thread(bridge.caller_thread)
            .map(|thread| thread.task);
        (bridge.return_.clone(), arguments, results, caller)
    };

    // The function is the caller's code, so the callee's scope comes
    // off the stack for the crossing and the caller's task is what
    // the crossing counts against. For a synchronous lower the
    // caller is already the scope under the callee; a callee resumed
    // from a queued item has nothing of the caller's under it, and
    // the caller's scope is pushed for the crossing alone.
    let pushed = {
        let mut guard = lock(tables)?;
        guard.leave_task_scope(task);
        match caller {
            Some(caller) if guard.tasks.current_task() != Some(caller) => {
                guard.tasks.push_task_scope(caller);
                true
            }
            _ => false,
        }
    };
    let crossed = return_
        .call(store_ctx.as_context_mut(), &arguments, &mut results)
        .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)));
    {
        // The callee's scope goes back whichever way the crossing
        // went: its core function or its callback is still on the
        // stack below this frame.
        let mut guard = lock(tables)?;
        if pushed {
            guard.tasks.pop_scope();
        }
        guard.tasks.push_task_scope(task);
    }
    crossed?;

    let mut guard = lock(tables)?;
    if let Some(bridge) = guard
        .tasks
        .subtask_mut(subtask)
        .and_then(|record| record.bridge.as_mut())
    {
        bridge.flat_results = results;
    }
    guard.tasks.subtask_returned(subtask)?;
    guard
        .tasks
        .task_mut(task)
        .ok_or_else(|| Error::internal("the current task has no record"))?
        .resolve(None);
    Ok(())
}

/// A zero of one flat type, which is what a result slot the callee
/// has yet to fill holds.
fn zero(ty: FlatType) -> RuntimeVal {
    match ty {
        FlatType::I32 => RuntimeVal::I32(0),
        FlatType::I64 => RuntimeVal::I64(0),
        FlatType::F32 => RuntimeVal::F32(0.0),
        FlatType::F64 => RuntimeVal::F64(0.0),
    }
}

/// Store `value` as the task's result, once the lift has produced it.
/// A task that has already resolved and a task that still owes a
/// borrow both trap here rather than resolving.
fn resolve(
    tables: &Arc<Mutex<HandleTables>>,
    task: TaskId,
    value: Option<Val>,
    result: Option<&ValueType>,
) -> Result<()> {
    let mut guard = lock(tables)?;
    let record = guard
        .tasks
        .task(task)
        .ok_or_else(|| Error::internal("the current task has no record"))?;
    if record.state == TaskState::Resolved {
        return Err(Error::Task(TaskCause::ReturnedTwice));
    }
    if record.num_borrows > 0 {
        return Err(outstanding_borrows(record.num_borrows, result));
    }
    guard
        .tasks
        .task_mut(task)
        .ok_or_else(|| Error::internal("the current task has no record"))?
        .resolve(value);
    Ok(())
}

/// Which of the three comparisons the built-in fails, or `None`
/// when it passes all three. The order is the reference's: the
/// result type, then the string encoding, then the memory.
fn mismatch(
    result_matches: bool,
    declared: &CanonOptions,
    lift_options: &CanonOptions,
) -> Option<ReturnMismatchKind> {
    if !result_matches {
        Some(ReturnMismatchKind::ResultType)
    } else if declared.string_encoding != lift_options.string_encoding {
        Some(ReturnMismatchKind::StringEncoding)
    } else if !same_memory(declared.memory, lift_options.memory) {
        Some(ReturnMismatchKind::Memory)
    } else {
        None
    }
}

/// Whether the memory the built-in's options name is the memory the
/// task's lift named.
///
/// The comparison is of runtime memory slots, not of memory
/// instances. `wasmtime_environ::component::Translator` interns a
/// slot on the core export a memory is extracted from — the slots
/// are that translator's, not the polyfill's — so two options that
/// name the same slot always name the same memory instance: the
/// check never lets a real mismatch through. It is conservative
/// in the other direction. A core module that re-exports a memory
/// it imported gives the same memory instance two core exports,
/// and so two slots; a built-in that names one of them while the
/// task's lift names the other traps with the return-mismatch
/// cause here, where Wasmtime's comparison of memory pointers
/// would pass. No component the translator accepts today produces
/// that shape.
///
/// Options that name no memory pass, because validation lets the
/// built-in leave the memory out only when the result needs none.
fn same_memory(builtin: Option<usize>, lift: Option<usize>) -> bool {
    match builtin {
        None => true,
        Some(slot) => lift == Some(slot),
    }
}

/// Whether the flattened result travels through one pointer rather
/// than in the built-in's parameter slots. The result values are the
/// built-in's parameters, so the limit is the flat-parameter limit of
/// sixteen and not the one slot a function that returns its result
/// has.
fn spills(ty: &ValueType) -> bool {
    !matches!(flat_count(ty), Some(count) if count <= MAX_FLAT_PARAMS)
}

/// The pointer a guest passes when the flattened result spills: the
/// built-in's one argument.
fn pointer_argument(args: &[RuntimeVal]) -> Result<usize> {
    match args.first() {
        Some(RuntimeVal::I32(pointer)) => Ok(*pointer as u32 as usize),
        _ => Err(Error::internal(
            "`task.return` was called without the pointer its core signature declares",
        )),
    }
}

/// The outstanding-borrows failure of a scope exit, labelled with
/// the built-in's result type when it has one. A built-in that
/// returns nothing labels the failure with no type: the borrow is
/// owed by the task, not by a value the return was processing.
fn outstanding_borrows(count: u32, result: Option<&ValueType>) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Result,
        valtype: result.cloned(),
        cause: AbiCause::OutstandingBorrows {
            count: count as usize,
        },
    })
}

/// Lock the store's records.
fn lock(tables: &Arc<Mutex<HandleTables>>) -> Result<MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))
}

#[cfg(test)]
mod tests {
    use wasm_runtime_layer::{Memory, MemoryType};

    use super::*;
    use crate::abi::layout::{FlatType, flat_types};
    use crate::component::FunctionType;
    use crate::concurrency::{InstanceId, TaskResult};
    use crate::engine::Engine;
    use crate::executor::ir::{CoreParameter, DataModel, StringEncoding};
    use crate::resource::TableId;
    use crate::store::Store;
    use crate::types::{PrimitiveType, TupleType};

    /// One store as a built-in reaches it: the store itself, its
    /// records, the canonical-ABI runtime state of one
    /// instantiation, and the identity of the one component instance
    /// both the records and that state name.
    struct Records {
        store: Store<()>,
        tables: Arc<Mutex<HandleTables>>,
        abi_state: Arc<Mutex<AbiRuntimeState>>,
        instance: InstanceId,
    }

    impl Records {
        /// A store holding one component instance and `memories`
        /// freshly created memories, one per runtime memory slot.
        fn new(memories: usize) -> Self {
            let engine = Engine::new().expect("engine");
            let mut store: Store<()> = Store::new(&engine, ()).expect("store");
            let tables = store.tables_handle();
            let instance = tables.lock().expect("records").tasks.insert_instance();
            let abi_state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
                memories,
                0,
                0,
                0,
                Vec::new(),
                vec![instance],
                vec![TableId::fresh()],
            )));
            for slot in 0..memories {
                let memory = Memory::new(store.inner_mut(), MemoryType::new(1, None))
                    .expect("a fresh memory");
                abi_state.lock().expect("runtime state").memories[slot] = Some(memory);
            }
            Self {
                store,
                tables,
                abi_state,
                instance,
            }
        }

        /// Create the task of a call into an `async` export whose
        /// result is `result` and whose lift declared `lift`, and
        /// push it as the current scope.
        fn push_task(&self, result: Option<ValueType>, lift: CanonOptions) -> TaskId {
            self.tables.lock().expect("records").tasks.push_task(
                Some(FunctionType {
                    parameters: Vec::new(),
                    result,
                    async_: true,
                }),
                Some(lift),
                self.instance,
            )
        }

        /// Build the built-in a `canon task.return (result $result)`
        /// with options `options` declares, and call it with `args`.
        fn call(
            &mut self,
            result: Option<ValueType>,
            options: &CanonOptions,
            args: &[RuntimeVal],
        ) -> anyhow::Result<()> {
            // The core signature the translator records: the
            // flattened result as parameters, or one pointer when it
            // spills.
            let signature = CoreSignature {
                params: match &result {
                    None => Vec::new(),
                    Some(ty) if spills(ty) => vec![CoreParameter::Value(FlatType::I32)],
                    Some(ty) => flat_types(ty)
                        .into_iter()
                        .map(CoreParameter::Value)
                        .collect(),
                },
                results: Vec::new(),
            };
            let abi_state = self.abi_state.clone();
            let func = build_task_return(
                &mut self.store.context(),
                result,
                0,
                options,
                &signature,
                abi_state,
            );
            func.call(self.store.inner_mut(), args, &mut [])
        }

        /// What the task resolved with, or `None` while it is
        /// pending.
        fn resolved(&self, task: TaskId) -> Option<Option<Val>> {
            let guard = self.tables.lock().expect("records");
            match &guard.tasks.task(task).expect("task record").result {
                TaskResult::Returned(value) => Some(value.clone()),
                _ => None,
            }
        }
    }

    /// The canon options of a lift or of the built-in: `memory` is
    /// the runtime memory slot the options name, `encoding` the
    /// string encoding, and `async_` whether the `async` option is
    /// declared.
    fn options(memory: Option<usize>, encoding: StringEncoding, async_: bool) -> CanonOptions {
        CanonOptions {
            instance: 0,
            memory,
            realloc: None,
            post_return: None,
            async_,
            callback: None,
            string_encoding: encoding,
            data_model: DataModel::LinearMemory,
        }
    }

    /// The options of an `async` lift that reads memory slot 0 and
    /// encodes strings as UTF-8, which every test below starts from.
    fn lift_options() -> CanonOptions {
        options(Some(0), StringEncoding::Utf8, true)
    }

    /// The `u32` result type most of the tests below return.
    fn u32_type() -> ValueType {
        ValueType::Primitive(PrimitiveType::U32)
    }

    /// Whether the whole chain of `err` mentions `message`. A trap a
    /// built-in raises reaches the caller through the substrate,
    /// which adds context of its own around it.
    fn reports(err: anyhow::Error, message: &str) -> bool {
        format!("{err:?}").contains(message)
    }

    /// Wasmtime's message for a `task.return` whose result type or
    /// options do not match the task's lift. The conformance corpus
    /// matches the trap by this substring, so a test of one
    /// comparison asserts it as well as the comparison's own words.
    const MISMATCH: &str = "invalid `task.return` signature and/or options for current task";

    /// The whole message the mismatch of `kind` renders as: the
    /// message above and the comparison that failed. Every mismatch
    /// message still carries Wasmtime's words, which is what the
    /// corpus matches.
    fn mismatch_message(kind: ReturnMismatchKind) -> String {
        let message = Error::Task(TaskCause::ReturnMismatch { kind }).to_string();
        assert!(
            message.contains(MISMATCH),
            "the corpus matches the mismatch trap by Wasmtime's words, which {message:?} no \
             longer carries"
        );
        message
    }

    #[wcmp_macros::test]
    fn it_resolves_the_current_task_with_the_lifted_result() {
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());

        records
            .call(
                Some(u32_type()),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(7)],
            )
            .expect("the built-in resolves the task");

        assert_eq!(
            records.resolved(task),
            Some(Some(Val::U32(7))),
            "the task holds the result the built-in lifted"
        );
    }

    #[wcmp_macros::test]
    fn it_lifts_a_string_result_through_the_tasks_memory() {
        // A string result is a pointer and a length into the task's
        // memory, so the crossing the built-in builds has to read
        // that memory and not merely carry the two slots.
        let mut records = Records::new(1);
        let string = ValueType::Primitive(PrimitiveType::String);
        let task = records.push_task(Some(string.clone()), lift_options());
        {
            let state = records.abi_state.lock().expect("runtime state");
            let memory = state.memories[0].as_ref().expect("memory slot 0");
            memory
                .write(records.store.inner_mut(), 8, b"hi")
                .expect("write the string into the guest's memory");
        }

        records
            .call(
                Some(string),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(8), RuntimeVal::I32(2)],
            )
            .expect("the built-in resolves the task");

        assert_eq!(
            records.resolved(task),
            Some(Some(Val::String("hi".to_owned()))),
            "the string was read out of the task's memory"
        );
    }

    #[wcmp_macros::test]
    fn it_lifts_a_spilled_result_from_one_pointer() {
        // Seventeen `u32` fields exceed the flat-parameter limit of
        // sixteen, so the built-in takes one pointer to the result
        // in the task's memory instead of the flattened values.
        let mut records = Records::new(1);
        let wide = ValueType::Tuple(TupleType::new((0..17).map(|_| u32_type())));
        let task = records.push_task(Some(wide.clone()), lift_options());
        {
            let state = records.abi_state.lock().expect("runtime state");
            let memory = state.memories[0].as_ref().expect("memory slot 0");
            for field in 0..17u32 {
                memory
                    .write(
                        records.store.inner_mut(),
                        16 + field as usize * 4,
                        &field.to_le_bytes(),
                    )
                    .expect("write the tuple into the guest's memory");
            }
        }

        records
            .call(
                Some(wide),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(16)],
            )
            .expect("the built-in resolves the task");

        let expected: Vec<Val> = (0..17).map(Val::U32).collect();
        assert_eq!(
            records.resolved(task),
            Some(Some(Val::Tuple(expected.into_boxed_slice()))),
            "every field was read out of the task's memory through the pointer"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_result_type_differs_from_the_tasks() {
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());

        let err = records
            .call(
                Some(ValueType::Primitive(PrimitiveType::U64)),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I64(7)],
            )
            .expect_err("a `u64` result does not resolve a `u32` task");

        assert!(
            reports(err, &mismatch_message(ReturnMismatchKind::ResultType)),
            "the return-mismatch cause, naming the result-type comparison"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_string_encoding_differs_from_the_tasks() {
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());

        let err = records
            .call(
                Some(u32_type()),
                &options(Some(0), StringEncoding::Utf16, false),
                &[RuntimeVal::I32(7)],
            )
            .expect_err("the built-in's encoding is not the lift's");

        assert!(
            reports(err, &mismatch_message(ReturnMismatchKind::StringEncoding)),
            "the return-mismatch cause, naming the string-encoding comparison"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_memory_differs_from_the_tasks() {
        // Two memories, one per runtime slot: the task's lift reads
        // the first and the built-in names the second.
        let mut records = Records::new(2);
        let task = records.push_task(Some(u32_type()), lift_options());

        let err = records
            .call(
                Some(u32_type()),
                &options(Some(1), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(7)],
            )
            .expect_err("the built-in's memory is not the task's");

        assert!(
            reports(err, &mismatch_message(ReturnMismatchKind::Memory)),
            "the return-mismatch cause, naming the memory comparison"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_built_in_returns_nothing_and_the_task_expects_a_result() {
        // The result-type comparison is of two `Option`s, so a
        // built-in declared with no result at all fails it against a
        // task whose function has one.
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());

        let err = records
            .call(None, &options(None, StringEncoding::Utf8, false), &[])
            .expect_err("no result does not resolve a task that expects one");

        assert!(
            reports(err, &mismatch_message(ReturnMismatchKind::ResultType)),
            "the return-mismatch cause, naming the result-type comparison"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_built_in_returns_a_result_and_the_task_expects_none() {
        // The reverse: the task's function returns nothing, so a
        // built-in that carries a result fails the same comparison.
        let mut records = Records::new(1);
        let task = records.push_task(None, lift_options());

        let err = records
            .call(
                Some(u32_type()),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(7)],
            )
            .expect_err("a `u32` result does not resolve a task that expects none");

        assert!(
            reports(err, &mismatch_message(ReturnMismatchKind::ResultType)),
            "the return-mismatch cause, naming the result-type comparison"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_fails_the_lift_when_options_that_name_no_memory_carry_a_result_that_needs_one() {
        // Validation lets a `task.return` leave the memory out only
        // when its result needs none, so this built-in is one no
        // component produces, and the three implementations part
        // ways on it. The reference traps at the memory comparison.
        // Wasmtime lifts the string through the task's own memory
        // and succeeds. The polyfill passes the comparison — it
        // lets options that name no memory through — and then
        // faults out of bounds in the lift, which has no memory to
        // read from. The shape is validation-illegal, so nothing
        // observes the difference.
        let mut records = Records::new(1);
        let string = ValueType::Primitive(PrimitiveType::String);
        let task = records.push_task(Some(string.clone()), lift_options());

        let err = records
            .call(
                Some(string),
                &options(None, StringEncoding::Utf8, false),
                &[RuntimeVal::I32(8), RuntimeVal::I32(2)],
            )
            .expect_err("the string cannot be read without a memory");

        assert!(
            reports(err, "out-of-bounds memory access"),
            "the lift reports the absent memory as a read it cannot serve"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }

    #[wcmp_macros::test]
    fn it_returns_through_options_that_name_no_memory() {
        // Validation lets a `task.return` leave the memory out when
        // its result needs none, and such a built-in passes the
        // memory comparison even though the task's lift names one.
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());

        records
            .call(
                Some(u32_type()),
                &options(None, StringEncoding::Utf8, false),
                &[RuntimeVal::I32(7)],
            )
            .expect("the built-in resolves the task");

        assert_eq!(
            records.resolved(task),
            Some(Some(Val::U32(7))),
            "the result needs no memory, so naming none is no mismatch"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_task_has_already_returned() {
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());
        let declared = options(Some(0), StringEncoding::Utf8, false);

        records
            .call(Some(u32_type()), &declared, &[RuntimeVal::I32(7)])
            .expect("the first return resolves the task");
        let err = records
            .call(Some(u32_type()), &declared, &[RuntimeVal::I32(8)])
            .expect_err("the second return is refused");

        assert!(
            reports(
                err,
                "`task.return` or `task.cancel` called more than once for current task"
            ),
            "the returned-twice cause"
        );
        assert_eq!(
            records.resolved(task),
            Some(Some(Val::U32(7))),
            "the first result stands"
        );
    }

    #[wcmp_macros::test]
    fn it_traps_when_the_task_still_owes_a_borrow() {
        // The scope-exit rule of a borrow applies to a return as it
        // applies to every other end of a call, and it refuses a
        // task that returns nothing as readily as one that returns
        // a value. The failure of such a task processes no value
        // type — the borrow is owed by the task, not by a value the
        // return was lifting — so it carries none and the rendering
        // leaves the type label out.
        let mut records = Records::new(1);
        let task = records.push_task(None, lift_options());
        records
            .tables
            .lock()
            .expect("records")
            .tasks
            .task_mut(task)
            .expect("task record")
            .num_borrows = 1;

        let err = records
            .call(None, &options(Some(0), StringEncoding::Utf8, false), &[])
            .expect_err("the outstanding borrow refuses the return");

        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("borrow handles outstanding"),
            "the outstanding-borrows cause, but the failure rendered as {rendered:?}"
        );
        assert!(
            !rendered.contains("(type "),
            "a return that carries no result names no value type, but the failure \
             rendered as {rendered:?}"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }
}
