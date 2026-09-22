//! Host trampoline construction.
//!
//! When the executor encounters a lowered import, it must build a
//! runtime-layer [`Func`] that the guest calls as if it were a
//! core-Wasm function. Inside that function:
//!
//! 1. The flat core-Wasm arguments are *lifted* through the canonical
//!    ABI into polyfill [`Val`]s using the lowering's canon options.
//!    When the parameter tuple is too wide for flat passing, the
//!    guest passes one pointer and the arguments are lifted from
//!    linear memory instead.
//! 2. The host-registered [`HostFunc<T>`] payload is invoked with
//!    those `Val`s.
//! 3. The host's `Val` result is *lowered* back into core-Wasm flat
//!    slots, or written into the return area the caller supplied
//!    when the result is too wide for flat passing.
//!
//! A lowered import leaves the component instance, so the call is
//! refused before any of that happens when the instance's may-leave
//! flag is clear, which is the case while a `cabi_realloc` or a
//! `post-return` the polyfill called runs. The reference traps in
//! `canon_lower` on the same condition, and the flag is the core
//! global the instance's adapters compile against, so a guest reads
//! one flag whichever way it tries to leave.
//!
//! Both crossings run on a boundary context the trampoline builds
//! from the lowering's canon options, resolved at call time against
//! the instantiation's
//! [`AbiRuntimeState`](crate::abi::runtime_state::AbiRuntimeState) —
//! wasmtime emits `LowerImport` before the
//! `ExtractMemory`/`Realloc`/`PostReturn` that depend on the same
//! module the lowered import is passed into, so the slots are filled
//! between trampoline construction and the trampoline's first call.
//!
//! That is the shape of a synchronous lower. A lowered import whose
//! `canon lower` declares the `async` option presents a different
//! core type to the guest — at most four flat parameters, the result
//! always through a return-area pointer, and one `i32` result, the
//! status word — and gives the guest control back before the callee
//! returns. The arguments cross exactly as above; what differs is
//! what the trampoline answers with. It answers with the status
//! word, which is the state of the call's subtask and, when the call
//! has not finished, the index of the entry the guest waits on.
//!
//! Such a lower always reaches a concurrent registration. The
//! `async` option may only be used with an async function type, so a
//! component that lowers a sync-typed import with it is refused
//! where the component is read, before any registration is
//! consulted; and the link rule holds an async-typed import to a
//! concurrent registration. The two rules meet at the same place: a
//! synchronous registration is out of reach through an asynchronous
//! lower, and the trampoline carries no path for that pairing.
//!
//! The registration answers with the future of one call: the
//! trampoline builds a host task from that future, with a lowering
//! that writes the result through the boundary context of the
//! subtask, and hands it to the store. The store polls it once. A
//! future that is ready lowers its result there and then, and the
//! guest sees the returned state. A future that is not joins the
//! store's host tasks, its subtask enters the caller's handle table,
//! and the guest sees the started state with that index; a later
//! turn lowers the result, resolves the subtask, and fills the
//! subtask event that `waitable-set.wait` and `waitable-set.poll`
//! deliver.
//!
//! The call that produces that future runs inside a poll scope of the
//! store, so the registration's closure reaches the store through the
//! accessor it is handed before the future exists. That is how a host
//! carries a piece of the store into an `async` block, which borrows
//! nothing: the value is read out in the closure and moved in.
//!
//! The opposite pairing — a concurrent registration reached through
//! a *synchronous* lower — is reachable, because the two axes move
//! separately: the link rule holds a concurrent registration to an
//! async-typed import, and a guest may lower an async-typed import
//! without the `async` option. The guest expects the result when the
//! call returns, so the trampoline blocks the guest thread on the
//! future where it stands, through the suspend seam, and writes the
//! result exactly where a synchronous registration's crossing writes
//! it.
//!
//! [`Func`]: wasm_runtime_layer::Func
//! [`HostFunc<T>`]: crate::linker::HostFunc

use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow};
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, Val as RuntimeVal, ValType as CoreType,
};

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::context::BoundaryContext;
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::instance::BoundaryInstance;
use crate::abi::instance_flags::InstanceFlags;
use crate::abi::layout::{
    FlatType, MAX_FLAT_ASYNC_PARAMS, flat_param_count, flat_types, params_spill, result_spills,
    spill_layout,
};
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift, lower};
use crate::backend::Backend;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, TaskCause};
use crate::executor::ir::{CanonOptions, LoweringSpec};
use crate::internal::{
    AccessorInternal, ErrorInternal, HostCallInternal, HostResourceInternal, ResourceTypeIdInternal,
};
use crate::linker::{HostCall, HostFuncFuture, HostFuncKind, HostResource};
use crate::store::StoreContextInternalExt;

use super::ResourceDestructor;
use crate::concurrency::{
    Accessor, CallStatus, HostTask, InstanceId, LowerKind, PollScope, Scope, SubtaskId,
    SubtaskState,
};
use crate::resource::{HandleKind, HandleTables, ResourceTableRuntime, ResourceTypeId, TableId};
use crate::store::{StoreContext, StoreData};
use crate::types::{PrimitiveType, ResourceType, TupleType, ValueType};
use crate::value::Val;

/// What the `resource.drop` trampoline says about a destructor that
/// failed, as the context over the destructor's own error rather
/// than as a rendering of it.
const DESTRUCTOR_FAILED: &str = "resource destructor failed";

/// Per-resource runtime data captured by every resource trampoline.
///
/// Bundles the engine-issued identity of the resource type with the
/// host destructor closure. The `Arc` shape is preserved so the
/// resource trampolines for `new`/`drop`/`rep` — which all touch the
/// same handle table — share one ledger.
pub struct ResourceRuntime<T> {
    /// The identity of the resource type: the host registration's
    /// for an imported resource, a fresh one per instantiation for a
    /// locally-defined resource. It names no table. The store keys
    /// the host's own table for the type by it, and keys the
    /// destructor recorded at instantiation by it, and every handle
    /// lookup passes it as the type check the entry has to match.
    pub type_id: ResourceTypeId,
    /// The name an error about a handle of this resource renders,
    /// for a resource a host registration carries: the label the
    /// registration was made under. A resource the component defines
    /// carries none here — nothing outside the binary names it — and
    /// the instantiation reads its name off the component's own
    /// resource tables instead.
    pub name: Option<ResourceType>,
    /// The destructor invoked when the guest drops the last handle
    /// to a resource.
    pub destructor: ResourceDestructor<T>,
}

impl<T> ResourceRuntime<T> {
    /// Construct a runtime bundle from a host registration carrier
    /// and the label the registration was found under.
    pub fn from_registration(host: &HostResource<T>, label: &str) -> Self {
        Self {
            type_id: host.type_id(),
            name: Some(ResourceType::new(label)),
            destructor: ResourceDestructor::Host(host.destructor().clone()),
        }
    }

    /// Construct a runtime bundle for a resource `instance` defines:
    /// a fresh identity and an empty destructor slot that the
    /// `DefineResource` directive fills.
    pub fn local(instance: InstanceId) -> Self {
        Self {
            type_id: ResourceTypeId::fresh(),
            name: None,
            destructor: ResourceDestructor::Local {
                function: Arc::new(Mutex::new(None)),
                instance,
            },
        }
    }
}

impl<T> Clone for ResourceRuntime<T> {
    fn clone(&self) -> Self {
        Self {
            type_id: self.type_id,
            name: self.name.clone(),
            destructor: self.destructor.clone(),
        }
    }
}

/// Build a runtime-layer host function that implements the
/// canonical `resource.drop` intrinsic for a single resource type.
///
/// The returned function takes one i32 (the handle index), removes
/// the entry from the handle table the owning component instance
/// keeps, and runs the host destructor with the entry's rep.
/// Surfaces a structured ABI error if the index does not address a
/// live entry.
///
/// The destructor runs as a task with one fresh thread, which is the
/// current scope until it returns or fails: the reference lifts the
/// destructor as a synchronous function of one `u32` parameter and
/// lowers a call to it. The thread's context slots start at zero and
/// end with it, so the destructor sees zeros and what it sets does
/// not reach the thread that dropped the handle. A destructor that
/// drops another resource nests a second task the same way.
///
/// The built-in leaves the component instance, so it is refused
/// while `flags` — the may-leave flag of the instance whose table
/// the handle is in — is clear. That instance is the calling one,
/// and its flag is the only one either reference reads, including
/// when another component instance defines the resource and so
/// supplies the destructor.
///
/// In the spec's `canon_resource_drop`
/// (`design/mvp/canonical-abi/definitions.py`) the trap on entry
/// reads the calling instance, and the destructor then goes out
/// through `inst.store.lower(callee, ft, opts, inst)`, lowered for
/// that same calling instance, so the `canon_lower` it reaches
/// traps on the caller's flag a second time. The defining instance
/// is reached through the `lift` beside that `lower`, which gates
/// on may-enter rather than may-leave. Wasmtime agrees: right
/// before the destructor it calls `check_may_leave_instance` on
/// `self.types[resource].unwrap_concrete_instance()`
/// (`crates/cranelift/src/compiler/component.rs`), which is the
/// instance the handle table belongs to and so is the caller again;
/// the `!= def.instance` test around it only skips that second read
/// when the resource is the caller's own.
pub fn build_resource_drop_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    table: ResourceTableRuntime,
    runtime: ResourceRuntime<T>,
    flags: InstanceFlags,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let func_type = FuncType::new([CoreType::I32], []);
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        func_type,
        move |mut store_ctx, args, _results| {
            refuse_unless_may_leave(&flags, &mut store_ctx)?;
            let index = take_i32(args, 0).map_err(|err| anyhow!("resource.drop: {err}"))?;
            // Dropping a borrow returns it to its call and runs no
            // destructor; dropping an owned entry runs the destructor,
            // unless a borrow of it is still lent to the host.
            let Some(rep) = drop_handle(&tables, table, index)? else {
                return Ok(());
            };
            // The task is entered before the destructor and ends
            // with the guard, whether the destructor returned or
            // failed.
            let _call = BoundaryCall::destructor(&tables, runtime.destructor.instance())?;
            // A destructor's failure is carried, not rendered. The
            // call this drop is inside reports whatever the
            // trampoline hands back, and a host that wants to know
            // what went wrong can read a structured error and cannot
            // read a string. `context` leaves the error it wraps
            // reachable, so a failure the browser backend raised —
            // its refusal of a re-entrant host call, which a
            // destructor that drops a second handle of its own
            // resource type meets — still downcasts out of the
            // `substrate_failure` the export's own call site applies,
            // and the host reads the cause that names the limitation
            // rather than a substrate failure.
            match &runtime.destructor {
                ResourceDestructor::Host(body) => body(store_ctx.data_mut().host_mut(), rep)
                    .map_err(|err| anyhow::Error::new(err).context(DESTRUCTOR_FAILED))?,
                ResourceDestructor::Local { function, .. } => {
                    let destructor = function
                        .lock()
                        .map_err(|_| anyhow!("resource destructor slot poisoned"))?
                        .clone();
                    if let Some(destructor) = destructor {
                        destructor
                            .call(&mut store_ctx, &[RuntimeVal::I32(rep as i32)], &mut [])
                            .context(DESTRUCTOR_FAILED)?;
                    }
                }
            }
            Ok(())
        },
    )
}

/// Build a runtime-layer host function that implements the
/// canonical `resource.new` intrinsic for a single resource type.
/// The returned function takes one i32 (the rep) and returns the
/// minted index. It leaves the component instance, so it is refused
/// while `flags` — the may-leave flag of the instance whose table
/// the handle enters — is clear.
pub fn build_resource_new_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    table: ResourceTableRuntime,
    flags: InstanceFlags,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        func_type,
        move |mut store_ctx, args, results| {
            refuse_unless_may_leave(&flags, &mut store_ctx)?;
            let rep = take_i32(args, 0).map_err(|err| anyhow!("resource.new: {err}"))?;
            let index = insert_handle(&tables, table, rep)?;
            results[0] = RuntimeVal::I32(index as i32);
            Ok(())
        },
    )
}

/// Build a runtime-layer host function that implements the
/// canonical `resource.rep` intrinsic for a single resource type.
/// The returned function takes one i32 (the handle index) and
/// returns the rep stored at that entry.
pub fn build_resource_rep_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    table: ResourceTableRuntime,
) -> RuntimeFunc {
    let tables = store.internal().tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.internal().runtime_mut(),
        func_type,
        move |_store_ctx, args, results| {
            let index = take_i32(args, 0).map_err(|err| anyhow!("resource.rep: {err}"))?;
            let rep = read_handle(&tables, table, index)?;
            results[0] = RuntimeVal::I32(rep as i32);
            Ok(())
        },
    )
}

/// Refuse a lowered import while the component instance it belongs
/// to may not be left.
fn trap_if_cannot_leave(instance: &BoundaryInstance, store: impl AsContextMut) -> Result<()> {
    let Some(flags) = instance.flags() else {
        return Err(Error::internal(
            "a lowered import names a component instance with no may-leave flag of its own",
        ));
    };
    refuse_unless_may_leave(flags, store)
}

/// Refuse a built-in that leaves the component instance while the
/// instance's may-leave flag is clear, which is the case while a
/// `cabi_realloc` or a `post-return` of that instance runs. The flag
/// is the core global the instance's adapters compile against, so
/// this reads what the generated code reads.
fn refuse_unless_may_leave(flags: &InstanceFlags, store: impl AsContextMut) -> Result<()> {
    if flags.may_leave(store)? {
        return Ok(());
    }
    Err(Error::Task(TaskCause::CannotLeave))
}

fn take_i32(args: &[RuntimeVal], cursor: usize) -> Result<u32> {
    match args.get(cursor) {
        Some(RuntimeVal::I32(v)) => Ok(*v as u32),
        _ => Err(Error::internal(
            "resource trampoline expected an i32 argument",
        )),
    }
}

/// Remove the entry at `index` for a `resource.drop`. Returns the rep
/// of an owned entry, whose destructor the caller runs, or `None`
/// for a borrow, which is handed back to the call it belongs to.
pub fn drop_handle(
    tables: &Arc<Mutex<HandleTables>>,
    table: ResourceTableRuntime,
    index: u32,
) -> Result<Option<u32>> {
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let entry = guard
        .lookup(table.table, index, table.type_id, table.guest_defined)
        .map_err(|e| invalid_handle_reason(e.to_string()))?;
    match entry {
        HandleKind::Own {
            lend_count: 0, rep, ..
        } => {
            guard.for_table_mut(table.table).remove(index);
            Ok(Some(rep))
        }
        HandleKind::Own { .. } => Err(invalid_handle_reason(
            "cannot remove owned resource while borrowed".to_owned(),
        )),
        HandleKind::Borrow { task, .. } => {
            if !guard.return_borrow(task) {
                return Err(invalid_handle(index));
            }
            guard.for_table_mut(table.table).remove(index);
            Ok(None)
        }
        _ => unreachable!("lookup only ever returns a resource entry"),
    }
}

pub fn insert_handle(
    tables: &Arc<Mutex<HandleTables>>,
    table: ResourceTableRuntime,
    rep: u32,
) -> Result<u32> {
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    Ok(guard.insert_own(table.table, table.type_id, table.guest_defined, rep))
}

fn read_handle(
    tables: &Arc<Mutex<HandleTables>>,
    table: ResourceTableRuntime,
    index: u32,
) -> Result<u32> {
    let guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    guard
        .lookup(table.table, index, table.type_id, table.guest_defined)
        .map(|entry| {
            entry
                .rep()
                .expect("lookup only ever returns a resource entry")
        })
        .map_err(|e| invalid_handle_reason(e.to_string()))
}

fn invalid_handle_reason(reason: String) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(0),
        valtype: Some(ValueType::Primitive(PrimitiveType::U32)),
        cause: AbiCause::InvalidHandle { reason },
    })
}

fn invalid_handle(index: u32) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(0),
        valtype: Some(ValueType::Primitive(PrimitiveType::U32)),
        cause: AbiCause::InvalidHandle {
            reason: format!("unknown handle index {index}"),
        },
    })
}

/// Build a runtime-layer host function that implements the lowered
/// import described by `spec`, dispatching to `host_func` — the
/// registration's kind, which says whether the call runs a closure to
/// completion or starts a host task — and drawing memory/realloc from
/// `abi_state` at call time.
pub fn build_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    spec: &LoweringSpec,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    host_func: HostFuncKind<T>,
) -> RuntimeFunc {
    let func_type = derive_runtime_func_type(&spec.signature, spec.kind);
    let signature = spec.signature.clone();
    let options = spec.options.clone();
    let kind = spec.kind;
    let tables = store.internal().tables_handle();

    RuntimeFunc::new(
        store.internal().runtime_mut(),
        func_type,
        move |store_ctx, args, results| {
            invoke_trampoline(
                store_ctx, &signature, &options, kind, &abi_state, &tables, &host_func, args,
                results,
            )
            .map_err(|err| anyhow!("trampoline invocation failed: {err}"))
        },
    )
}

/// Derive the core-Wasm function type the lowered import presents
/// to the guest, from the component-level `signature` and the kind
/// of lowering the `canon lower` declared. The two kinds flatten the
/// same type differently, so the kind — and not the `async` effect
/// the type carries — is what picks the rule.
///
/// A synchronous lower flattens the parameters and the result per
/// the canonical ABI's ordinary limits: a parameter tuple wider than
/// `MAX_FLAT_PARAMS` collapses to one `i32` pointer, and a result
/// wider than `MAX_FLAT_RESULTS` adds an `i32` return-area pointer
/// as the final parameter instead of being returned.
///
/// An asynchronous lower keeps at most `MAX_FLAT_ASYNC_PARAMS`
/// flat parameters and takes one `i32` pointer instead when the
/// flattened parameters exceed that; it never returns the result,
/// which always travels through a return-area pointer appended to
/// the parameters; and it returns one `i32`, the status word that
/// names the subtask's state and its index.
fn derive_runtime_func_type(signature: &FunctionType, kind: LowerKind) -> FuncType {
    match kind {
        LowerKind::Sync => sync_runtime_func_type(signature),
        LowerKind::Async => async_runtime_func_type(signature),
    }
}

/// The core type of a synchronous lower.
fn sync_runtime_func_type(signature: &FunctionType) -> FuncType {
    let mut params: Vec<CoreType> = Vec::new();
    if params_spill(signature) {
        params.push(CoreType::I32);
    } else {
        params.extend(flat_parameters(signature));
    }

    let mut results: Vec<CoreType> = Vec::new();
    if let Some(result_ty) = &signature.result {
        if result_spills(signature) {
            params.push(CoreType::I32);
        } else {
            for slot in flat_types(result_ty) {
                results.push(core_type_of_flat(slot));
            }
        }
    }

    FuncType::new(params, results)
}

/// The core type of an asynchronous lower.
fn async_runtime_func_type(signature: &FunctionType) -> FuncType {
    let mut params: Vec<CoreType> = Vec::new();
    if async_params_spill(signature) {
        params.push(CoreType::I32);
    } else {
        params.extend(flat_parameters(signature));
    }

    // The reference spells the return-area test as a flattened
    // width greater than zero, but no component type flattens to no
    // slots at all: a record, tuple, variant, flags or enum must
    // carry at least one member, and a fixed-length list at least
    // one element. So presence of a result is the same test, and it
    // is the one the validator the guest module is checked against
    // applies. Testing the width here would derive one parameter
    // fewer than that validator expects if a zero-slot type ever
    // became legal.
    if signature.result.is_some() {
        params.push(CoreType::I32);
    }

    FuncType::new(params, [CoreType::I32])
}

/// Whether the parameter tuple of an asynchronous lower spills into
/// linear memory rather than travelling in flat slots.
fn async_params_spill(signature: &FunctionType) -> bool {
    !matches!(flat_param_count(signature), Some(n) if n <= MAX_FLAT_ASYNC_PARAMS)
}

/// The core types of the signature's parameters, in order, as they
/// are passed when the tuple does not spill.
fn flat_parameters(signature: &FunctionType) -> Vec<CoreType> {
    signature
        .parameters
        .iter()
        .flat_map(|p| flat_types(&p.ty))
        .map(core_type_of_flat)
        .collect()
}

fn core_type_of_flat(slot: FlatType) -> CoreType {
    match slot {
        FlatType::I32 => CoreType::I32,
        FlatType::I64 => CoreType::I64,
        FlatType::F32 => CoreType::F32,
        FlatType::F64 => CoreType::F64,
    }
}

/// What one call of the host side of a lowered import produced: the
/// values a synchronous registration's closure returned, or the
/// future a concurrent registration answered with.
enum HostOutcome {
    /// A synchronous registration ran its closure to completion. The
    /// vector holds one value when the type declares a result and
    /// none otherwise.
    Values(Vec<Val>),
    /// A concurrent registration answered with the future of this one
    /// call. The store owns it as a host task from here on.
    Future(HostFuncFuture),
}

/// The body of a trampoline closure. Reads the per-call canon
/// options state, lifts arguments, dispatches to the host func, and
/// answers with the return — the lowered result of a synchronous
/// lower, or the status word of an asynchronous one.
#[allow(clippy::too_many_arguments)]
fn invoke_trampoline<T: 'static>(
    mut store_ctx: wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    declared: &CanonOptions,
    kind: LowerKind,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    host_func: &HostFuncKind<T>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
    // The canon options of the lowering and the instance they name,
    // read out of the instance's runtime state under one lock of it.
    // Each crossing of the call builds its boundary context from the
    // two, and the instance is where the handle tables of the
    // crossing come from.
    let (options, instance) = BoundaryInstance::resolve(declared, abi_state, tables)?;

    // A lowered import leaves the component instance, so it is
    // refused while the instance may not be left: the reference
    // traps in `canon_lower` on the same condition, and the flag is
    // clear for the length of a `cabi_realloc` or a `post-return`
    // the polyfill called. The check comes before anything of the
    // call happens, so a refused call lifts no argument and pushes
    // no subtask.
    trap_if_cannot_leave(&instance, &mut store_ctx)?;

    // A call from the guest into the host is a subtask: it goes on
    // the stack of current scopes and stays there while the host side
    // runs. Borrows the guest lends in are recorded against it, and
    // are given back when the subtask's resolution is delivered.
    let subtask = lock_tables(tables)?.tasks.push_subtask();

    // Lifting the parameters and running the host function both
    // happen with the subtask on the stack, and either can fail. The
    // failure travels past the pop that would have ended the subtask,
    // so the whole of it is one fallible step whose one error path
    // ends the subtask below.
    let called = (|store_ctx: &mut wasm_runtime_layer::StoreContextMut<
        '_,
        StoreData<T>,
        Backend,
    >|
     -> Result<(HostOutcome, Option<usize>)> {
        let mut cursor = 0usize;
        let mut lift_ctx = BoundaryContext::new(
            store_ctx.as_context_mut(),
            options.clone(),
            instance.clone(),
            Some(Scope::Subtask(subtask)),
        );
        let lifted = if parameters_spill(signature, kind) {
            lift_spilled_arguments(&mut lift_ctx, signature, args, &mut cursor)?
        } else {
            let mut lifted: Vec<Val> = Vec::with_capacity(signature.parameters.len());
            for (i, param) in signature.parameters.iter().enumerate() {
                let position = AbiPosition::Argument(i);
                lifted.push(lift_from_flat_slots(
                    &mut lift_ctx,
                    args,
                    &mut cursor,
                    &param.ty,
                    position,
                )?);
            }
            lifted
        };

        // A synchronous lower passes a return-area pointer as the
        // final argument only when the result is too wide for flat
        // slots; an asynchronous lower always passes one, because it
        // never returns the result at all.
        let return_area_ptr = match &signature.result {
            Some(result_ty) if result_travels_through_memory(signature, kind) => Some(
                pointer_argument(args, &mut cursor, result_ty, AbiPosition::Result)?,
            ),
            _ => None,
        };

        // Drop the lift context borrow before invoking the host.
        drop(lift_ctx);

        // The parameters are lifted, so the callee has started.
        lock_tables(tables)?.tasks.start_subtask(subtask);

        let outcome = match host_func {
            HostFuncKind::Synchronous(body) => {
                let host_arity = usize::from(signature.result.is_some());
                let mut host_results: Vec<Val> = vec![Val::Bool(false); host_arity];
                // The host function runs against the whole store: the
                // polyfill's own state rides in the core store's data,
                // so the context the runtime layer handed this
                // trampoline reaches the scheduler, the suspend seam,
                // and the host tasks from inside the guest call, with
                // nothing captured.
                let call = HostCall::new(
                    StoreContext::new(store_ctx.as_context_mut()),
                    instance.resource_tables().to_vec(),
                );
                body(call, &lifted, &mut host_results)?;
                HostOutcome::Values(host_results)
            }
            // A concurrent registration's body runs only far enough
            // to obtain the future of this one call. It is handed a
            // token for the store rather than a borrow of it, because
            // the future outlives this frame: the store owns it and
            // polls it, and it reaches the store again only inside a
            // poll.
            //
            // That run is itself a poll of the store, so it happens
            // inside a scope of its own. A registration whose closure
            // reads the host data before it builds its future — the
            // plain way to carry a piece of the store into an `async`
            // block — reaches the store through the token it is
            // handed, exactly as the future reaches it from a later
            // poll; without the scope such a reach would find an
            // empty slot and fail. The waker is the running turn's,
            // or one that does nothing when no turn is running, which
            // is the waker the first poll of the host task takes as
            // well.
            HostFuncKind::Concurrent(start) => {
                let mut store = StoreContext::new(store_ctx.as_context_mut());
                let accessor: Accessor<T> = Accessor::new(store.id());
                let waker = store.active_waker();
                let started = {
                    let _poll = PollScope::enter(&mut store, &waker);
                    start(&accessor, lifted)
                };
                HostOutcome::Future(started)
            }
        };
        Ok((outcome, return_area_ptr))
    })(&mut store_ctx);

    let (outcome, return_area_ptr) = match called {
        Ok(outcome) => outcome,
        Err(err) => {
            // The call never returned, so the subtask's resolution is
            // a cancellation, and the handles the guest lent for it
            // are given back all the same. The lock is taken without
            // the usual error wrapping so that a poisoned lock does
            // not displace the failure that is being reported.
            if let Ok(mut guard) = tables.lock() {
                guard.abandon_subtask(subtask);
            }
            return Err(err);
        }
    };

    match outcome {
        HostOutcome::Values(host_results) => return_host_values(
            &mut store_ctx,
            signature,
            tables,
            options,
            instance,
            subtask,
            host_results,
            return_area_ptr,
            results,
        ),
        // Which lower the guest called through decides what becomes
        // of the future: an asynchronous lower hands the call back as
        // a subtask, and a synchronous one blocks the guest thread on
        // it until it resolves.
        HostOutcome::Future(future) => match kind {
            LowerKind::Async => start_host_call(
                &mut store_ctx,
                signature,
                declared,
                abi_state,
                options,
                instance,
                subtask,
                future,
                return_area_ptr,
                results,
            ),
            LowerKind::Sync => block_on_host_call(
                &mut store_ctx,
                signature,
                declared,
                abi_state,
                tables,
                options,
                instance,
                subtask,
                future,
                return_area_ptr,
                results,
            ),
        },
    }
}

/// Finish a call whose host side ran to completion: the closure of a
/// synchronous registration, which only a synchronous lower reaches.
/// The `async` canonical option is valid only on an async function
/// type, and the link rule holds an async-typed import to a
/// concurrent registration, so no component can pair a synchronous
/// registration with an asynchronous lower. There is therefore no
/// status word to answer here, and no lower to distinguish.
///
/// The subtask resolves before the results are written back: such a
/// call delivers its resolution as it returns, which gives back every
/// handle the guest lent for it. A borrow the host lowers back out
/// belongs to the caller's task, which is why the subtask leaves the
/// stack first, and why the crossing of the result counts against the
/// scope the pop uncovers.
#[allow(clippy::too_many_arguments)]
fn return_host_values<T: 'static>(
    store_ctx: &mut wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    tables: &Arc<Mutex<HandleTables>>,
    options: BoundaryOptions,
    instance: BoundaryInstance,
    subtask: SubtaskId,
    host_results: Vec<Val>,
    return_area_ptr: Option<usize>,
    results: &mut [RuntimeVal],
) -> Result<()> {
    let caller = {
        let mut guard = lock_tables(tables)?;
        guard.exit_subtask(subtask, SubtaskState::Returned);
        guard.tasks.current_scope()
    };
    write_host_result(
        store_ctx,
        signature,
        options,
        instance,
        caller,
        host_results,
        return_area_ptr,
        results,
    )
}

/// Write what the host side of one call produced into the guest.
///
/// Both callers are synchronous lowers: a synchronous registration's
/// closure, which an asynchronous lower cannot reach, and the block a
/// synchronous lower makes on a concurrent registration's future. So
/// the result goes where a synchronous lower expects it — through the
/// return area the guest passed when the result is too wide for flat
/// slots, and into the flat result slots otherwise — and there is no
/// status word to write. An asynchronous lower's result is written by
/// the host task's own lowering instead, in the turn that resolves
/// the subtask.
///
/// The subtask of the call has left the stack by the time this runs,
/// so `caller` is the scope the pop uncovered and the crossing of the
/// result counts against it: a borrow the host lowers back out
/// belongs to the caller's task.
#[allow(clippy::too_many_arguments)]
fn write_host_result<T: 'static>(
    store_ctx: &mut wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    options: BoundaryOptions,
    instance: BoundaryInstance,
    caller: Option<Scope>,
    host_results: Vec<Val>,
    return_area_ptr: Option<usize>,
    results: &mut [RuntimeVal],
) -> Result<()> {
    let Some(result_ty) = &signature.result else {
        return Ok(());
    };
    // Neither caller can arrive here with an empty vector. A
    // synchronous registration writes into a slice this trampoline
    // sized from the declared signature, so its length is one
    // wherever there is a result at all. A concurrent registration's
    // vector is checked a call earlier: the untyped entry,
    // `LinkerInstance::func_new_concurrent`, wraps the future in the
    // check that fails a vector the declared type does not describe,
    // and the typed entry derives the vector from the closure's
    // return, so its length holds by construction. That is the check
    // a mistaken host reads. An empty vector here is therefore a
    // broken invariant rather than a host's mistake, and it reads as
    // one.
    let host_val = host_results.into_iter().next().ok_or_else(|| {
        Error::internal("a host registration's result never reached the crossing")
    })?;
    let mut lower_ctx = BoundaryContext::new(store_ctx.as_context_mut(), options, instance, caller);
    match return_area_ptr {
        Some(ptr) => lower(
            &mut lower_ctx,
            ptr,
            &host_val,
            result_ty,
            AbiPosition::Result,
        )?,
        None => {
            let mut slots: Vec<RuntimeVal> = Vec::new();
            lower_into_flat_slots(
                &mut lower_ctx,
                &host_val,
                result_ty,
                &mut slots,
                AbiPosition::Result,
            )?;
            if slots.len() != results.len() {
                return Err(Error::from(AbiError {
                    position: AbiPosition::Result,
                    valtype: Some(result_ty.clone()),
                    cause: AbiCause::InvalidEncoding {
                        message: format!(
                            "lowered {} flat result slots for a core signature with {}",
                            slots.len(),
                            results.len()
                        ),
                    },
                }));
            }
            for (dst, src) in results.iter_mut().zip(slots) {
                *dst = src;
            }
        }
    }
    Ok(())
}

/// Start a call whose host side answered with a future: a concurrent
/// registration reached through an asynchronous lower.
///
/// The future becomes a host task, with a lowering that writes the
/// result through the boundary context of this call's subtask. The
/// store polls it once and says what the guest is told: the returned
/// state when the future was ready, and the started state with the
/// subtask's index in the caller's handle table when it was not.
#[allow(clippy::too_many_arguments)]
fn start_host_call<T: 'static>(
    store_ctx: &mut wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    declared: &CanonOptions,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    options: BoundaryOptions,
    instance: BoundaryInstance,
    subtask: SubtaskId,
    future: HostFuncFuture,
    return_area_ptr: Option<usize>,
    results: &mut [RuntimeVal],
) -> Result<()> {
    let caller_table = caller_handle_table(abi_state, declared.instance)?;
    let result_ty = signature.result.clone();
    // The lowering carries what the crossing of the result needs,
    // because it runs in a turn of its own: the call the guest made
    // has returned by then, so there is no frame left to read the
    // options, the instance, or the return area back from.
    let lowering = move |store: &mut StoreContext<'_, T>, produced: Result<Vec<Val>>| {
        let values = produced?;
        let Some(result_ty) = result_ty else {
            return Ok(());
        };
        let ptr = return_area_ptr.ok_or_else(|| {
            Error::internal("an asynchronous lower with a result kept no return area")
        })?;
        // The arity of a concurrent registration's value vector is
        // that registration's own contract, and it is checked once,
        // where the registration is made: the untyped entry wraps
        // the future in the check that fails a vector the declared
        // type does not describe, and the typed entry derives the
        // vector from the closure's return, so its length holds by
        // construction. That is the check that fires, and a mistaken
        // host reads it. A future therefore cannot reach this
        // lowering with a vector of the wrong length, so an empty
        // one here is a broken invariant rather than a host's
        // mistake, and it reads as one.
        let host_val = values.into_iter().next().ok_or_else(|| {
            Error::internal("a concurrent registration's future produced no value to lower")
        })?;
        let mut lower_ctx = BoundaryContext::new(
            store.internal().runtime_mut().as_context_mut(),
            options,
            instance,
            Some(Scope::Subtask(subtask)),
        );
        lower(
            &mut lower_ctx,
            ptr,
            &host_val,
            &result_ty,
            AbiPosition::Result,
        )
    };

    let task = HostTask::from_future(subtask, lowering, future);
    let mut store = StoreContext::new(store_ctx.as_context_mut());
    let status = store
        .internal()
        .start_host_task(task, caller_table, LowerKind::Async)?;
    write_status(results, status)
}

/// Block on a call whose host side answered with a future: a
/// concurrent registration reached through a synchronous lower.
///
/// The guest expects the result when the call returns, so the store
/// blocks the guest thread on the future where it stands. The block
/// runs through the suspend seam and polls the future at every check
/// of its condition, so a future that resolves after a few polls
/// resolves inside it; a future that stays pending fails the call
/// with the cause the seam selects, and the failure travels out to
/// the guest's call.
///
/// The whole of the call is therefore over by the time the block
/// returns: the subtask has resolved, which gave back the handles the
/// guest lent for it, and the lowering has carried what the body
/// produced back to this frame. What is left is the crossing of the
/// result, which runs exactly where a synchronous registration's
/// crossing runs.
#[allow(clippy::too_many_arguments)]
fn block_on_host_call<T: 'static>(
    store_ctx: &mut wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    declared: &CanonOptions,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    options: BoundaryOptions,
    instance: BoundaryInstance,
    subtask: SubtaskId,
    future: HostFuncFuture,
    return_area_ptr: Option<usize>,
    results: &mut [RuntimeVal],
) -> Result<()> {
    let caller_table = caller_handle_table(abi_state, declared.instance)?;
    // The call resolves in this frame, so its lowering has nothing to
    // carry but the values themselves: the options, the instance and
    // the return area are all still here to cross with.
    let produced: Arc<Mutex<Option<Vec<Val>>>> = Arc::new(Mutex::new(None));
    let slot = produced.clone();
    let lowering = move |_store: &mut StoreContext<'_, T>, outcome: Result<Vec<Val>>| {
        *lock_produced(&slot)? = Some(outcome?);
        Ok(())
    };

    let task = HostTask::from_future(subtask, lowering, future);
    {
        let mut store = StoreContext::new(store_ctx.as_context_mut());
        // The status a synchronous lower comes back with is always
        // the returned state, since the call is over: what the guest
        // is told is the result itself, written below.
        store
            .internal()
            .start_host_task(task, caller_table, LowerKind::Sync)?;
    }

    let caller = lock_tables(tables)?.tasks.current_scope();
    let host_results = lock_produced(&produced)?.take().ok_or_else(|| {
        Error::internal("a synchronous lower's block returned without the call's result")
    })?;
    write_host_result(
        store_ctx,
        signature,
        options,
        instance,
        caller,
        host_results,
        return_area_ptr,
        results,
    )
}

/// Borrow the slot a blocked synchronous lower leaves its result in.
fn lock_produced(
    slot: &Arc<Mutex<Option<Vec<Val>>>>,
) -> Result<std::sync::MutexGuard<'_, Option<Vec<Val>>>> {
    slot.lock()
        .map_err(|_| Error::internal("a synchronous lower's result slot is poisoned"))
}

/// The handle table of the component instance that made the call,
/// where a subtask the guest has to wait on gets its entry. The
/// lowering names the instance by the translator's per-instantiation
/// index, which is the same index the waitable built-ins use.
fn caller_handle_table(
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    instance: usize,
) -> Result<TableId> {
    let state = abi_state
        .lock()
        .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
    state.handle_tables.get(instance).copied().ok_or_else(|| {
        Error::internal(format!(
            "a lowered import named component instance {instance}, which this instantiation does \
             not hold"
        ))
    })
}

/// Write the status word an asynchronous lower answers with.
fn write_status(results: &mut [RuntimeVal], status: CallStatus) -> Result<()> {
    let Some(slot) = results.first_mut() else {
        return Err(Error::internal(
            "an asynchronous lower was built with no status word to return",
        ));
    };
    *slot = RuntimeVal::I32(status.value() as i32);
    Ok(())
}

/// Whether the parameter tuple travels through one pointer into
/// linear memory rather than in flat slots. The two lowerings measure
/// the same tuple against different limits.
fn parameters_spill(signature: &FunctionType, kind: LowerKind) -> bool {
    match kind {
        LowerKind::Sync => params_spill(signature),
        LowerKind::Async => async_params_spill(signature),
    }
}

/// Whether the result travels through a return-area pointer the guest
/// passes. A synchronous lower passes one only for a result too wide
/// for flat slots; an asynchronous lower never returns the result, so
/// it always passes one.
fn result_travels_through_memory(signature: &FunctionType, kind: LowerKind) -> bool {
    match kind {
        LowerKind::Sync => result_spills(signature),
        LowerKind::Async => true,
    }
}

/// Lift every parameter from the spilled tuple the guest wrote to
/// linear memory. The single flat argument is the tuple's address;
/// each parameter sits at the offset the canonical ABI's record
/// layout gives it.
///
/// The address is the guest's, so the whole tuple is gated before
/// the first parameter is read: aligned to the tuple's alignment,
/// and addressing a region of the tuple's size that the memory
/// owns. Gating parameter by parameter would let a tuple whose
/// first parameter is in bounds and whose last is not be lifted
/// half-way.
fn lift_spilled_arguments<T: 'static>(
    ctx: &mut BoundaryContext<'_, T>,
    signature: &FunctionType,
    args: &[RuntimeVal],
    cursor: &mut usize,
) -> Result<Vec<Val>> {
    let types: Vec<ValueType> = signature.parameters.iter().map(|p| p.ty.clone()).collect();
    let layout = spill_layout(&types);
    let first = types
        .first()
        .cloned()
        .unwrap_or(ValueType::Primitive(PrimitiveType::U32));
    let base = pointer_argument(args, cursor, &first, AbiPosition::Argument(0))?;
    // The tuple type labels a refusal and nothing else, so it is
    // built where the refusal is raised rather than on every call
    // that passes the gate.
    let fail = |message: &str| {
        Error::from(AbiError {
            position: AbiPosition::Argument(0),
            valtype: Some(ValueType::Tuple(TupleType::new(types.clone()))),
            cause: AbiCause::InvalidEncoding {
                message: message.to_owned(),
            },
        })
    };
    if !base.is_multiple_of(layout.alignment) {
        return Err(fail("pointer not aligned"));
    }
    if base.checked_add(layout.size).is_none() {
        return Err(fail("pointer size overflow"));
    }
    if !ctx.in_bounds(base, layout.size) {
        return Err(fail("pointer out of bounds"));
    }
    let mut lifted = Vec::with_capacity(types.len());
    for (i, (ty, offset)) in types.iter().zip(layout.offsets.iter()).enumerate() {
        lifted.push(lift(ctx, base + offset, ty, AbiPosition::Argument(i))?);
    }
    Ok(lifted)
}

/// Read one `i32` pointer argument at `cursor` and advance it.
fn pointer_argument(
    args: &[RuntimeVal],
    cursor: &mut usize,
    ty: &ValueType,
    position: AbiPosition,
) -> Result<usize> {
    match args.get(*cursor) {
        Some(RuntimeVal::I32(p)) => {
            *cursor += 1;
            Ok(*p as u32 as usize)
        }
        _ => Err(Error::from(AbiError {
            position,
            valtype: Some(ty.clone()),
            cause: AbiCause::InvalidEncoding {
                message: "expected an i32 pointer argument".to_owned(),
            },
        })),
    }
}

fn lock_tables(
    tables: &Arc<Mutex<HandleTables>>,
) -> Result<std::sync::MutexGuard<'_, HandleTables>> {
    tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::component::FunctionParameter;

    /// A signature of `count` `u32` parameters, each flattening to
    /// one `i32` slot, and the given result.
    fn signature(count: usize, result: Option<ValueType>) -> FunctionType {
        FunctionType {
            parameters: (0..count)
                .map(|i| FunctionParameter {
                    name: format!("p{i}"),
                    ty: ValueType::Primitive(PrimitiveType::U32),
                })
                .collect(),
            result,
            async_: false,
        }
    }

    /// A signature of the given parameter types, in order, and the
    /// given result, for the cases where a parameter flattens to
    /// more than one slot.
    fn signature_of(parameters: &[ValueType], result: Option<ValueType>) -> FunctionType {
        FunctionType {
            parameters: parameters
                .iter()
                .enumerate()
                .map(|(i, ty)| FunctionParameter {
                    name: format!("p{i}"),
                    ty: ty.clone(),
                })
                .collect(),
            result,
            async_: false,
        }
    }

    /// The `u32` result type the signatures below carry, which
    /// flattens to one slot.
    fn u32_result() -> Option<ValueType> {
        Some(ValueType::Primitive(PrimitiveType::U32))
    }

    /// The core type of a lowering, as the pair a reader compares.
    fn core_type(signature: &FunctionType, kind: LowerKind) -> (Vec<CoreType>, Vec<CoreType>) {
        let derived = derive_runtime_func_type(signature, kind);
        (derived.params().to_vec(), derived.results().to_vec())
    }

    #[wcmp_macros::test]
    fn it_keeps_the_flat_parameters_of_an_asynchronous_lower_within_four() {
        // Four `u32` parameters are four flat slots, the most an
        // asynchronous lower passes directly. The result is the
        // status word alone, because the type has no result.
        let (params, results) = core_type(&signature(4, None), LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32; 4]);
        assert_eq!(results, vec![CoreType::I32]);
    }

    #[wcmp_macros::test]
    fn it_spills_the_parameters_of_an_asynchronous_lower_beyond_four() {
        // The fifth slot puts the tuple over the limit, so the whole
        // of it travels through one pointer into linear memory —
        // where a synchronous lower would still pass five slots.
        let five = signature(5, None);
        let (params, results) = core_type(&five, LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32]);
        assert_eq!(results, vec![CoreType::I32]);

        let (sync_params, sync_results) = core_type(&five, LowerKind::Sync);
        assert_eq!(sync_params, vec![CoreType::I32; 5]);
        assert_eq!(sync_results, Vec::new());
    }

    #[wcmp_macros::test]
    fn it_measures_an_asynchronous_lower_in_flat_slots_not_in_parameters() {
        // A `string` flattens to two slots, a pointer and a length,
        // so three of them are six slots from three parameters. The
        // limit is on the slots, so the tuple spills even though the
        // parameters number fewer than four.
        let string = ValueType::Primitive(PrimitiveType::String);
        let three_strings = signature_of(&[string.clone(), string.clone(), string], None);

        let (params, results) = core_type(&three_strings, LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32]);
        assert_eq!(results, vec![CoreType::I32]);

        // Six slots are well within the synchronous limit, which is
        // what shows the three parameters really are six slots.
        let (sync_params, sync_results) = core_type(&three_strings, LowerKind::Sync);
        assert_eq!(sync_params, vec![CoreType::I32; 6]);
        assert_eq!(sync_results, Vec::new());
    }

    #[wcmp_macros::test]
    fn it_keeps_the_return_pointer_of_an_asynchronous_lower_out_of_the_limit() {
        // Four flat slots are the most that travel directly, and a
        // result adds the return-area pointer as a fifth parameter.
        // The pointer is not itself counted against the limit, so
        // the parameters stay flat rather than spilling.
        let (params, results) = core_type(&signature(4, u32_result()), LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32; 5]);
        assert_eq!(results, vec![CoreType::I32]);
    }

    #[wcmp_macros::test]
    fn it_returns_the_result_of_an_asynchronous_lower_through_a_pointer() {
        // One `u32` result fits a flat slot, and an asynchronous
        // lower returns it through a pointer all the same: the
        // pointer is the last parameter and the one `i32` result is
        // the status word.
        let (params, results) = core_type(&signature(1, u32_result()), LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32, CoreType::I32]);
        assert_eq!(results, vec![CoreType::I32]);

        // The pointer follows the spilled parameter pointer too, so
        // a wide call takes exactly two.
        let (params, results) = core_type(&signature(5, u32_result()), LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32, CoreType::I32]);
        assert_eq!(results, vec![CoreType::I32]);
    }

    #[wcmp_macros::test]
    fn it_gives_an_asynchronous_lower_without_a_result_no_return_pointer() {
        // With no result there is nothing to write back, so the
        // parameters stand alone and the status word is still the
        // one `i32` the call returns.
        let (params, results) = core_type(&signature(2, None), LowerKind::Async);
        assert_eq!(params, vec![CoreType::I32; 2]);
        assert_eq!(results, vec![CoreType::I32]);
    }

    #[wcmp_macros::test]
    fn it_leaves_the_signature_of_a_synchronous_lower_unchanged() {
        // The synchronous lower keeps the canonical ABI's ordinary
        // limits: up to sixteen flat parameters, the result in a
        // flat slot, and no status word.
        let (params, results) = core_type(&signature(5, u32_result()), LowerKind::Sync);
        assert_eq!(params, vec![CoreType::I32; 5]);
        assert_eq!(results, vec![CoreType::I32]);

        let (params, results) = core_type(&signature(0, None), LowerKind::Sync);
        assert_eq!(params, Vec::new());
        assert_eq!(results, Vec::new());

        // Seventeen slots are one too many, and the tuple spills.
        let (params, results) = core_type(&signature(17, None), LowerKind::Sync);
        assert_eq!(params, vec![CoreType::I32]);
        assert_eq!(results, Vec::new());
    }
}
