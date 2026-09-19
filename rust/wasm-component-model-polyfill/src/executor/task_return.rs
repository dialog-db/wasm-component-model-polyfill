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

use std::sync::{Arc, Mutex, MutexGuard};

use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, StoreContextMut as RuntimeContextMut, Val as RuntimeVal,
};

use crate::abi::context::BoundaryContext;
use crate::abi::flatten::lift_from_flat_slots;
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{MAX_FLAT_PARAMS, flat_count};
use crate::abi::lift;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::backend::Backend;
use crate::concurrency::{InstanceId, Scope, TaskId, TaskState};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, ReturnMismatchKind, TaskCause};
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
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    args: &[RuntimeVal],
) -> Result<()> {
    let (task, task_instance, may_leave) = current_task(tables)?;
    if !may_leave {
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
    if let Some(kind) = mismatch(declared, result, &context, &lift_options) {
        return Err(Error::Task(TaskCause::ReturnMismatch { kind }));
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

/// The task the built-in resolves, the instance that task belongs
/// to, and whether that instance may be left.
///
/// The task is the innermost one on the store's stack of current
/// scopes, which is the reference's current task. For a
/// `cabi_realloc` the polyfill called it is the realloc's own task,
/// whose instance may not be left, which is how the built-in refuses
/// a realloc that calls it. The one task that belongs to no
/// component instance, the destructor task of a resource the host
/// implements, runs host code only, so guest code never reaches the
/// built-in from it; that case is an internal error, not a trap.
fn current_task(tables: &Arc<Mutex<HandleTables>>) -> Result<(TaskId, InstanceId, bool)> {
    let guard = lock(tables)?;
    let task = guard
        .tasks
        .current_task()
        .ok_or_else(|| Error::internal("`task.return` ran with no task on the stack"))?;
    let instance = guard
        .tasks
        .task(task)
        .map(|record| record.instance)
        .ok_or_else(|| Error::internal("the current task has no record"))?
        .ok_or_else(|| Error::internal("the current task belongs to no component instance"))?;
    let may_leave = guard
        .tasks
        .instance(instance)
        .map(|record| record.may_leave)
        .ok_or_else(|| Error::internal("the current task names no instance record"))?;
    Ok((task, instance, may_leave))
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
fn mismatch<T: 'static>(
    declared: &CanonOptions,
    result: Option<&ValueType>,
    context: &BoundaryContext<'_, T>,
    lift_options: &CanonOptions,
) -> Option<ReturnMismatchKind> {
    if result.cloned() != context.task_result_type() {
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
/// check never lets a real mismatch through. It is conservative in the other direction. A
/// core module that re-exports a memory it imported gives the same
/// memory instance two core exports, and so two slots; a built-in
/// that names one of them while the task's lift names the other
/// traps with the return-mismatch cause here, where Wasmtime's
/// comparison of memory pointers would pass. No component the
/// translator accepts today produces that shape.
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
    use crate::executor::ir::{DataModel, StringEncoding};
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
                    Some(ty) if spills(ty) => vec![FlatType::I32],
                    Some(ty) => flat_types(ty),
                },
                results: Vec::new(),
            };
            let abi_state = self.abi_state.clone();
            let func = build_task_return(
                &mut self.store.context(),
                result,
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
        // component produces. It passes the memory comparison — the
        // comparison lets options that name no memory through — and
        // then fails in the lift, which has no memory to read the
        // string out of, rather than reading some other memory.
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
        // applies to every other end of a call.
        let mut records = Records::new(1);
        let task = records.push_task(Some(u32_type()), lift_options());
        records
            .tables
            .lock()
            .expect("records")
            .tasks
            .task_mut(task)
            .expect("task record")
            .num_borrows = 1;

        let err = records
            .call(
                Some(u32_type()),
                &options(Some(0), StringEncoding::Utf8, false),
                &[RuntimeVal::I32(7)],
            )
            .expect_err("the outstanding borrow refuses the return");

        assert!(
            reports(err, "borrow handles outstanding"),
            "the outstanding-borrows cause"
        );
        assert_eq!(records.resolved(task), None, "the task is still pending");
    }
}
