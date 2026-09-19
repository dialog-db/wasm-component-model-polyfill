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
//! Both crossings run on a boundary context the trampoline builds
//! from the lowering's canon options, resolved at call time against
//! the instantiation's
//! [`AbiRuntimeState`](crate::abi::runtime_state::AbiRuntimeState) —
//! wasmtime emits `LowerImport` before the
//! `ExtractMemory`/`Realloc`/`PostReturn` that depend on the same
//! module the lowered import is passed into, so the slots are filled
//! between trampoline construction and the trampoline's first call.
//!
//! [`Func`]: wasm_runtime_layer::Func
//! [`HostFunc<T>`]: crate::linker::HostFunc

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, Val as RuntimeVal, ValType as CoreType,
};

use crate::abi::boundary_call::BoundaryCall;
use crate::abi::context::BoundaryContext;
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{FlatType, flat_types, params_spill, result_spills, spill_layout};
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift, lower};
use crate::backend::Backend;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::{CanonOptions, LoweringSpec};
use crate::linker::{HostCall, HostFuncBody, HostResource};

use super::ResourceDestructor;
use crate::concurrency::{InstanceId, Scope, SubtaskState};
use crate::resource::{HandleKind, HandleTables, ResourceTableRuntime, ResourceTypeId};
use crate::store::{StoreContext, StoreData};
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

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
    /// The destructor invoked when the guest drops the last handle
    /// to a resource.
    pub destructor: ResourceDestructor<T>,
}

impl<T> ResourceRuntime<T> {
    /// Construct a runtime bundle from a host registration carrier.
    pub fn from_registration(host: &HostResource<T>) -> Self {
        Self {
            type_id: host.type_id,
            destructor: ResourceDestructor::Host(host.destructor.clone()),
        }
    }

    /// Construct a runtime bundle for a resource `instance` defines:
    /// a fresh identity and an empty destructor slot that the
    /// `DefineResource` directive fills.
    pub fn local(instance: InstanceId) -> Self {
        Self {
            type_id: ResourceTypeId::fresh(),
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
pub fn build_resource_drop_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    table: ResourceTableRuntime,
    runtime: ResourceRuntime<T>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], []);
    RuntimeFunc::new(
        store.runtime_mut(),
        func_type,
        move |mut store_ctx, args, _results| {
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
            match &runtime.destructor {
                ResourceDestructor::Host(body) => body(store_ctx.data_mut().host_mut(), rep)
                    .map_err(|err| anyhow!("resource destructor failed: {err}"))?,
                ResourceDestructor::Local { function, .. } => {
                    let destructor = function
                        .lock()
                        .map_err(|_| anyhow!("resource destructor slot poisoned"))?
                        .clone();
                    if let Some(destructor) = destructor {
                        destructor
                            .call(&mut store_ctx, &[RuntimeVal::I32(rep as i32)], &mut [])
                            .map_err(|err| anyhow!("resource destructor failed: {err}"))?;
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
/// minted index.
pub fn build_resource_new_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    table: ResourceTableRuntime,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.runtime_mut(),
        func_type,
        move |_store_ctx, args, results| {
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
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.runtime_mut(),
        func_type,
        move |_store_ctx, args, results| {
            let index = take_i32(args, 0).map_err(|err| anyhow!("resource.rep: {err}"))?;
            let rep = read_handle(&tables, table, index)?;
            results[0] = RuntimeVal::I32(rep as i32);
            Ok(())
        },
    )
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
/// import described by `spec`, dispatching to `host_func` and
/// drawing memory/realloc from `abi_state` at call time.
pub fn build_trampoline<T: 'static>(
    store: &mut StoreContext<'_, T>,
    spec: &LoweringSpec,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    host_func: Arc<HostFuncBody<T>>,
) -> RuntimeFunc {
    let func_type = derive_runtime_func_type(&spec.signature);
    let signature = spec.signature.clone();
    let options = spec.options.clone();
    let tables = store.tables_handle();

    RuntimeFunc::new(
        store.runtime_mut(),
        func_type,
        move |store_ctx, args, results| {
            invoke_trampoline(
                store_ctx,
                &signature,
                &options,
                &abi_state,
                &tables,
                host_func.as_ref(),
                args,
                results,
            )
            .map_err(|err| anyhow!("trampoline invocation failed: {err}"))
        },
    )
}

/// Derive the core-Wasm function type the lowered import presents
/// to the guest. The signature's parameters and result are flattened
/// per the canonical ABI. A parameter tuple wider than
/// `MAX_FLAT_PARAMS` collapses to one `i32` pointer; a result wider
/// than `MAX_FLAT_RESULTS` adds an `i32` return-area pointer as the
/// final parameter.
fn derive_runtime_func_type(signature: &FunctionType) -> FuncType {
    let mut params: Vec<CoreType> = Vec::new();
    if params_spill(signature) {
        params.push(CoreType::I32);
    } else {
        for p in &signature.parameters {
            for slot in flat_types(&p.ty) {
                params.push(core_type_of_flat(slot));
            }
        }
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

fn core_type_of_flat(slot: FlatType) -> CoreType {
    match slot {
        FlatType::I32 => CoreType::I32,
        FlatType::I64 => CoreType::I64,
        FlatType::F32 => CoreType::F32,
        FlatType::F64 => CoreType::F64,
    }
}

/// The body of a trampoline closure. Reads the per-call canon
/// options state, lifts arguments, dispatches to the host func, and
/// lowers the return.
#[allow(clippy::too_many_arguments)]
fn invoke_trampoline<T: 'static>(
    mut store_ctx: wasm_runtime_layer::StoreContextMut<'_, StoreData<T>, Backend>,
    signature: &FunctionType,
    options: &CanonOptions,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    host_func: &HostFuncBody<T>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
    // The canon options of the lowering and the instance they name,
    // read out of the instance's runtime state under one lock of it.
    // Each crossing of the call builds its boundary context from the
    // two, and the instance is where the handle tables of the
    // crossing come from.
    let (options, instance) = BoundaryInstance::resolve(options, abi_state, tables)?;

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
     -> Result<(Vec<Val>, Option<usize>)> {
        let mut cursor = 0usize;
        let mut lift_ctx = BoundaryContext::new(
            store_ctx.as_context_mut(),
            options.clone(),
            instance.clone(),
            Some(Scope::Subtask(subtask)),
        );
        let lifted = if params_spill(signature) {
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

        // When the result is too wide for flat slots, the caller
        // passes a return-area pointer as the final argument.
        let return_area_ptr = match &signature.result {
            Some(result_ty) if result_spills(signature) => Some(pointer_argument(
                args,
                &mut cursor,
                result_ty,
                AbiPosition::Result,
            )?),
            _ => None,
        };

        // Drop the lift context borrow before invoking the host.
        drop(lift_ctx);

        // The parameters are lifted, so the callee has started.
        lock_tables(tables)?.tasks.start_subtask(subtask);

        let host_arity = usize::from(signature.result.is_some());
        let mut host_results: Vec<Val> = vec![Val::Bool(false); host_arity];
        // The host function runs against the whole store: the
        // polyfill's own state rides in the core store's data, so
        // the context the runtime layer handed this trampoline
        // reaches the scheduler, the suspend seam, and the host
        // tasks from inside the guest call, with nothing captured.
        let call = HostCall::new(
            StoreContext::new(store_ctx.as_context_mut()),
            instance.resource_tables().to_vec(),
        );
        host_func(call, &lifted, &mut host_results)?;
        Ok((host_results, return_area_ptr))
    })(&mut store_ctx);

    let (host_results, return_area_ptr) = match called {
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

    // The success path resolves the subtask before results are
    // written back: a synchronous lower delivers the resolution as it
    // returns, which gives back every handle the guest lent for the
    // call. A borrow the host lowers back out belongs to the caller's
    // task, which is why the subtask leaves the stack first, and why
    // the crossing of the result counts against the scope the pop
    // uncovers.
    let caller = {
        let mut guard = lock_tables(tables)?;
        guard.exit_subtask(subtask, SubtaskState::Returned);
        guard.tasks.current_scope()
    };

    let Some(result_ty) = &signature.result else {
        return Ok(());
    };
    let host_val = host_results.into_iter().next().ok_or_else(|| {
        Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: Some(result_ty.clone()),
            cause: AbiCause::HostValueMismatch,
        })
    })?;
    let mut lower_ctx = BoundaryContext::new(store_ctx.as_context_mut(), options, instance, caller);
    match return_area_ptr {
        Some(ptr) => lower(
            &mut lower_ctx,
            ptr,
            &host_val,
            result_ty,
            AbiPosition::Result,
        ),
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
            Ok(())
        }
    }
}

/// Lift every parameter from the spilled tuple the guest wrote to
/// linear memory. The single flat argument is the tuple's address;
/// each parameter sits at the offset the canonical ABI's record
/// layout gives it.
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
