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
//! Memory, realloc, and post-return are looked up at call time from
//! a shared [`AbiRuntimeState`] populated by the executor's
//! `Extract*` directives — wasmtime emits `LowerImport` before the
//! `ExtractMemory`/`Realloc`/`PostReturn` that depend on the same
//! module the lowered import is passed into, so the slots are filled
//! between trampoline construction and the trampoline's first call.
//!
//! [`Func`]: wasm_runtime_layer::Func
//! [`HostFunc<T>`]: crate::linker::HostFunc

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use wasm_runtime_layer::{
    AsContextMut, Func as RuntimeFunc, FuncType, Memory, Val as RuntimeVal, ValType as CoreType,
};

use crate::abi::context::{LiftContext, LowerContext};
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::layout::{FlatType, flat_types, params_spill, result_spills, spill_layout};
use crate::abi::{lift, lower};
use crate::backend::Backend;
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::{CanonOptions, LoweringSpec};
use crate::linker::{HostFuncBody, HostResource};

use super::ResourceDestructor;
use crate::resource::{HandleTables, ResourceTypeId};
use crate::store::Store;
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Per-component canonical-ABI runtime state. Populated by the
/// executor's `Extract*` directives during instantiation; consulted
/// by trampolines at call time. Shared via `Arc<Mutex<...>>` so the
/// runtime layer's `Send + Sync` bound on `Func::new` is satisfied.
pub struct AbiRuntimeState {
    pub memories: Vec<Option<Memory>>,
    pub reallocs: Vec<Option<RuntimeFunc>>,
    pub post_returns: Vec<Option<RuntimeFunc>>,
    /// The handle-table identity of every resource of the component,
    /// by the translator's resource index. Imported resources carry
    /// the identity of their host registration; locally-defined ones
    /// carry an identity minted for this instantiation.
    pub resource_types: Vec<ResourceTypeId>,
}

/// Per-resource runtime data captured by every resource trampoline.
///
/// Bundles the engine-issued identity (the per-store handle table
/// key) with the host destructor closure. The `Arc` shape is
/// preserved so the resource trampolines for `new`/`drop`/`rep` —
/// which all touch the same handle table — share one ledger.
pub struct ResourceRuntime<T> {
    /// The identity of the resource type: the host registration's
    /// for an imported resource, a fresh one per instantiation for a
    /// locally-defined resource. Used as the key into
    /// [`HandleTables`](crate::resource::HandleTables).
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

    /// Construct a runtime bundle for a locally-defined resource: a
    /// fresh identity and an empty destructor slot that the
    /// `DefineResource` directive fills.
    pub fn local() -> Self {
        Self {
            type_id: ResourceTypeId::fresh(),
            destructor: ResourceDestructor::Local(Arc::new(Mutex::new(None))),
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

impl AbiRuntimeState {
    /// Construct a state with the requested slab sizes, every slot
    /// initially empty.
    pub fn with_slabs(
        num_memories: usize,
        num_reallocs: usize,
        num_post_returns: usize,
        resource_types: Vec<ResourceTypeId>,
    ) -> Self {
        Self {
            memories: vec![None; num_memories],
            reallocs: vec![None; num_reallocs],
            post_returns: vec![None; num_post_returns],
            resource_types,
        }
    }
}

/// Build a runtime-layer host function that implements the
/// canonical `resource.drop` intrinsic for a single resource type.
///
/// The returned function takes one i32 (the handle index), removes
/// the entry from the per-store handle table, and runs the host
/// destructor with the entry's rep. Surfaces a structured ABI error
/// if the index does not address a live entry.
pub fn build_resource_drop_trampoline<T: 'static>(
    store: &mut Store<T>,
    runtime: ResourceRuntime<T>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], []);
    RuntimeFunc::new(
        store.inner_mut(),
        func_type,
        move |mut store_ctx, args, _results| {
            let index = take_i32(args, 0).map_err(|err| anyhow!("resource.drop: {err}"))?;
            let rep = remove_handle(&tables, runtime.type_id, index)?;
            match &runtime.destructor {
                ResourceDestructor::Host(body) => body(store_ctx.data_mut(), rep)
                    .map_err(|err| anyhow!("resource destructor failed: {err}"))?,
                ResourceDestructor::Local(slot) => {
                    let destructor = slot
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
    store: &mut Store<T>,
    runtime: ResourceRuntime<T>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.inner_mut(),
        func_type,
        move |_store_ctx, args, results| {
            let rep = take_i32(args, 0).map_err(|err| anyhow!("resource.new: {err}"))?;
            let index = insert_handle(&tables, runtime.type_id, rep)?;
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
    store: &mut Store<T>,
    runtime: ResourceRuntime<T>,
) -> RuntimeFunc {
    let tables = store.tables_handle();
    let func_type = FuncType::new([CoreType::I32], [CoreType::I32]);
    RuntimeFunc::new(
        store.inner_mut(),
        func_type,
        move |_store_ctx, args, results| {
            let index = take_i32(args, 0).map_err(|err| anyhow!("resource.rep: {err}"))?;
            let rep = read_handle(&tables, runtime.type_id, index)?;
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

fn remove_handle(
    tables: &Arc<Mutex<HandleTables>>,
    type_id: ResourceTypeId,
    index: u32,
) -> Result<u32> {
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let table = guard.for_type_mut(type_id);
    table.remove(index).ok_or_else(|| invalid_handle(index))
}

fn insert_handle(
    tables: &Arc<Mutex<HandleTables>>,
    type_id: ResourceTypeId,
    rep: u32,
) -> Result<u32> {
    let mut guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    Ok(guard.for_type_mut(type_id).insert(rep))
}

fn read_handle(
    tables: &Arc<Mutex<HandleTables>>,
    type_id: ResourceTypeId,
    index: u32,
) -> Result<u32> {
    let guard = tables
        .lock()
        .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
    let table = guard
        .for_type(type_id)
        .ok_or_else(|| invalid_handle(index))?;
    table.get(index).ok_or_else(|| invalid_handle(index))
}

fn invalid_handle(index: u32) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(0),
        valtype: ValueType::Primitive(PrimitiveType::U32),
        cause: AbiCause::InvalidHandle {
            reason: format!("handle index {index} is not live in the resource table"),
        },
    })
}

/// Build a runtime-layer host function that implements the lowered
/// import described by `spec`, dispatching to `host_func` and
/// drawing memory/realloc from `abi_state` at call time.
pub fn build_trampoline<T: 'static>(
    store: &mut Store<T>,
    spec: &LoweringSpec,
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    host_func: Arc<HostFuncBody<T>>,
) -> RuntimeFunc {
    let func_type = derive_runtime_func_type(&spec.signature);
    let signature = spec.signature.clone();
    let options = spec.options.clone();
    let tables = store.tables_handle();

    RuntimeFunc::new(
        store.inner_mut(),
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
    mut store_ctx: wasm_runtime_layer::StoreContextMut<'_, T, Backend>,
    signature: &FunctionType,
    options: &CanonOptions,
    abi_state: &Arc<Mutex<AbiRuntimeState>>,
    tables: &Arc<Mutex<HandleTables>>,
    host_func: &HostFuncBody<T>,
    args: &[RuntimeVal],
    results: &mut [RuntimeVal],
) -> Result<()> {
    let (memory, realloc) = {
        let state = abi_state
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
        let memory = options
            .memory
            .and_then(|s| state.memories.get(s).and_then(|m| m.clone()));
        let realloc = options
            .realloc
            .and_then(|s| state.reallocs.get(s).and_then(|r| r.clone()));
        (memory, realloc)
    };

    let mut cursor = 0usize;
    let mut lift_ctx = LiftContext::new(
        store_ctx.as_context_mut(),
        memory.clone(),
        options.string_encoding,
        Some(tables.clone()),
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

    // When the result is too wide for flat slots, the caller passes
    // a return-area pointer as the final argument.
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

    let host_arity = usize::from(signature.result.is_some());
    let mut host_results: Vec<Val> = vec![Val::Bool(false); host_arity];
    host_func(store_ctx.data_mut(), &lifted, &mut host_results)?;

    let Some(result_ty) = &signature.result else {
        return Ok(());
    };
    let host_val = host_results.into_iter().next().ok_or_else(|| {
        Error::from(AbiError {
            position: AbiPosition::Result,
            valtype: result_ty.clone(),
            cause: AbiCause::HostValueMismatch,
        })
    })?;
    let mut lower_ctx = LowerContext::new(
        store_ctx.as_context_mut(),
        memory,
        realloc,
        options.string_encoding,
        Some(tables.clone()),
    );
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
                    valtype: result_ty.clone(),
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
    ctx: &mut LiftContext<'_, T>,
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
            valtype: ty.clone(),
            cause: AbiCause::InvalidEncoding {
                message: "expected an i32 pointer argument".to_owned(),
            },
        })),
    }
}
