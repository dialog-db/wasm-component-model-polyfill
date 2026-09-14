//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{AsContextMut, Val as RuntimeVal};

use crate::abi::context::{LiftContext, LowerContext};
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::layout::{flat_types, params_spill, result_spills, spill_layout};
use crate::abi::{lift, lower};
use crate::component::FunctionType;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result};
use crate::executor::ir::CanonOptions;
use crate::executor::trampoline::AbiRuntimeState;
use crate::store::{Store, StoreId};
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// A handle to one exported function of a component [`Instance`].
///
/// `Func` is obtained from [`Instance::get_func`] and is the unit a
/// caller invokes through. Calling drives the canonical-ABI
/// round-trip: lowers arguments through the export's canon options
/// (calling `cabi_realloc` for heap-allocating values), passes them
/// to the underlying core function, lifts the result back into the
/// polyfill's [`Val`] enum, and runs the export's `post-return`
/// after the caller observes the return.
///
/// [`Instance`]: super::Instance
/// [`Instance::get_func`]: super::Instance::get_func
pub struct Func {
    /// The leaf name this export was declared under. Carried so
    /// the typed-conversion entry point can name the export in
    /// type-mismatch diagnostics; `Func::call` does not consult it.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub name: String,
    /// The runtime-layer core-Wasm function handle this export
    /// resolves to. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub inner: wasm_runtime_layer::Func,
    /// The component-level signature the polyfill uses to lower
    /// arguments and lift results across the canonical-ABI
    /// boundary. Workspace-internal; never re-exported through
    /// `lib.rs`.
    pub signature: FunctionType,
    /// The canonical-ABI options the export's lift declared. Held
    /// here so [`Self::call`] can resolve memory/realloc/post-
    /// return at call time.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub options: CanonOptions,
    /// The instance's canonical-ABI runtime state. Shared with
    /// every host trampoline the same instance carries; the lock
    /// is taken briefly at the boundaries.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The identity of the [`Store`] the owning instance was
    /// created in. A call made through any other store is rejected
    /// before it reaches the runtime layer.
    /// Workspace-internal; never re-exported through `lib.rs`.
    pub store_id: StoreId,
}

impl Func {
    /// Invoke the function with the given polyfill-typed arguments.
    /// Drives the full canonical-ABI round-trip: heap-allocating
    /// arguments are lowered into guest memory via `cabi_realloc`,
    /// the underlying core function is called, the result is
    /// lifted, and `post-return` (when declared) is invoked once
    /// the caller has observed the return value.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in. Passing a different store returns
    /// [`InstantiationError::WrongStore`].
    pub fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
        if store.id != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        if args.len() != self.signature.parameters.len() {
            return Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: ValueType::Primitive(PrimitiveType::Bool),
                cause: AbiCause::InvalidEncoding {
                    message: format!(
                        "expected {} arguments, got {}",
                        self.signature.parameters.len(),
                        args.len()
                    ),
                },
            }));
        }

        // Resolve memory / realloc / post-return from the instance
        // state for the duration of the call.
        let (memory, realloc, post_return) = {
            let state = self
                .abi_state
                .lock()
                .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
            let memory = self
                .options
                .memory
                .and_then(|s| state.memories.get(s).and_then(|m| m.clone()));
            let realloc = self
                .options
                .realloc
                .and_then(|s| state.reallocs.get(s).and_then(|f| f.clone()));
            let post_return = self
                .options
                .post_return
                .and_then(|s| state.post_returns.get(s).and_then(|f| f.clone()));
            (memory, realloc, post_return)
        };

        let core_args = self.lower_args(store, args, memory.clone(), realloc.clone())?;
        let result_arity = self.core_result_arity();
        let mut core_results = vec![RuntimeVal::I32(0); result_arity];

        self.inner
            .call(store.inner_mut(), &core_args, &mut core_results)
            .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;

        let lifted_result = self.lift_result(store, &core_results, memory)?;

        // Run post-return (if any) after the caller has logically
        // observed the return; we hold the lifted value, so the
        // post-return is safe to call now. Its arguments are the
        // core results: the flat result slots, or the return-area
        // pointer when the result spilled to memory.
        if let Some(post_return_func) = post_return {
            let mut empty: [RuntimeVal; 0] = [];
            post_return_func
                .call(store.inner_mut(), &core_results, &mut empty)
                .map_err(|err| {
                    Error::from(AbiError {
                        position: AbiPosition::Result,
                        valtype: ValueType::Primitive(PrimitiveType::Bool),
                        cause: AbiCause::SubstrateFailure(err),
                    })
                })?;
        }

        Ok(lifted_result.into_iter().collect())
    }

    /// The number of core-Wasm result slots the underlying core
    /// function returns. Mirrors the rule
    /// [`crate::executor::trampoline`] uses to derive the core
    /// function type from the polyfill's signature.
    fn core_result_arity(&self) -> usize {
        match &self.signature.result {
            None => 0,
            Some(_) if result_spills(&self.signature) => 1,
            Some(result_ty) => flat_types(result_ty).len(),
        }
    }

    fn lower_args<T: 'static>(
        &self,
        store: &mut Store<T>,
        args: &[Val],
        memory: Option<wasm_runtime_layer::Memory>,
        realloc: Option<wasm_runtime_layer::Func>,
    ) -> Result<Vec<RuntimeVal>> {
        let tables = store.tables_handle();
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lower_ctx = LowerContext::new(
            store_ctx,
            memory,
            realloc,
            self.options.string_encoding,
            Some(tables),
        );

        if params_spill(&self.signature) {
            // The whole parameter tuple is written into guest
            // memory at the canonical ABI's record layout, and the
            // core function receives its address.
            let types: Vec<ValueType> = self
                .signature
                .parameters
                .iter()
                .map(|p| p.ty.clone())
                .collect();
            let layout = spill_layout(&types);
            let spill_ty = ValueType::Primitive(PrimitiveType::U32);
            let base = if layout.size == 0 {
                0
            } else {
                lower_ctx.allocate_aligned(
                    layout.size,
                    layout.alignment,
                    &spill_ty,
                    AbiPosition::Argument(0),
                )?
            };
            for (i, ((ty, val), offset)) in types
                .iter()
                .zip(args.iter())
                .zip(layout.offsets.iter())
                .enumerate()
            {
                lower(
                    &mut lower_ctx,
                    base + offset,
                    val,
                    ty,
                    AbiPosition::Argument(i),
                )?;
            }
            return Ok(vec![RuntimeVal::I32(base as i32)]);
        }

        let mut out: Vec<RuntimeVal> = Vec::new();
        for (i, (param, val)) in self
            .signature
            .parameters
            .iter()
            .zip(args.iter())
            .enumerate()
        {
            lower_into_flat_slots(
                &mut lower_ctx,
                val,
                &param.ty,
                &mut out,
                AbiPosition::Argument(i),
            )?;
        }
        Ok(out)
    }

    fn lift_result<T: 'static>(
        &self,
        store: &mut Store<T>,
        core_results: &[RuntimeVal],
        memory: Option<wasm_runtime_layer::Memory>,
    ) -> Result<Option<Val>> {
        let Some(result_ty) = &self.signature.result else {
            return Ok(None);
        };
        let position = AbiPosition::Result;
        let tables = store.tables_handle();
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lift_ctx = LiftContext::new(
            store_ctx,
            memory,
            self.options.string_encoding,
            Some(tables),
        );
        if result_spills(&self.signature) {
            // Wide result: read from the pointer the core function
            // returned.
            let ptr = match core_results.first() {
                Some(RuntimeVal::I32(p)) => *p as u32 as usize,
                _ => {
                    return Err(Error::from(AbiError {
                        position,
                        valtype: result_ty.clone(),
                        cause: AbiCause::InvalidEncoding {
                            message: "missing or non-i32 result-pointer slot".to_owned(),
                        },
                    }));
                }
            };
            return Ok(Some(lift(&mut lift_ctx, ptr, result_ty, position)?));
        }
        let mut cursor = 0usize;
        let val = lift_from_flat_slots(
            &mut lift_ctx,
            core_results,
            &mut cursor,
            result_ty,
            position,
        )?;
        Ok(Some(val))
    }
}
