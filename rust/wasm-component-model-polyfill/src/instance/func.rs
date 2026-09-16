//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{AsContextMut, Val as RuntimeVal};

use crate::abi::context::BoundaryContext;
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::layout::{flat_types, params_spill, result_spills, spill_layout};
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift, lower};
use crate::component::FunctionType;
use crate::concurrency::{InstanceId, Scope, TaskId};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result};
use crate::executor::ir::CanonOptions;
use crate::resource::ResourceTableRuntime;
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
    /// The component-level signature of the export: its parameters
    /// and its result, as the component declares them.
    pub fn ty(&self) -> &FunctionType {
        &self.signature
    }

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
    ///
    /// The future completes without suspending on both targets
    /// today; it is awaited so that an export which yields to the
    /// host can do so without a change of signature.
    pub async fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
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

        // The canon options of the export's lift, resolved against
        // the instance's runtime state once for the whole call. Each
        // crossing of the call builds its boundary context from
        // them.
        let options = BoundaryOptions::resolve(&self.options, &self.abi_state)?;
        let instance = options.instance().ok_or_else(|| {
            Error::internal("an export's lift names a component instance the plan does not hold")
        })?;

        // A call from the host into the guest is a task: it goes on
        // the stack of current scopes. Borrows the host lowers in are
        // owed to it and must be dropped by the guest before the call
        // ends; borrows the guest lifts out in results lend to it
        // until the call ends.
        let task =
            store.enter_export_task(self.signature.clone(), self.options.clone(), instance)?;
        let outcome = self.call_in_task(task, instance, store, args, &options);
        match outcome {
            Ok(result) => {
                store.resolve_export_task(task, result.first().cloned())?;
                if let Err(count) = store.exit_export_task(task)? {
                    return Err(Error::from(AbiError {
                        position: AbiPosition::Result,
                        valtype: ValueType::Primitive(PrimitiveType::Bool),
                        cause: AbiCause::OutstandingBorrows {
                            count: count as usize,
                        },
                    }));
                }
                Ok(result)
            }
            Err(err) => {
                store.abandon_export_task(task)?;
                Err(err)
            }
        }
    }

    /// The body of [`Self::call`] inside its task.
    fn call_in_task<T: 'static>(
        &self,
        task: TaskId,
        instance: InstanceId,
        store: &mut Store<T>,
        args: &[Val],
        options: &BoundaryOptions,
    ) -> Result<Box<[Val]>> {
        let core_args = self.lower_args(store, args, instance, task, options)?;
        let result_arity = self.core_result_arity();
        let mut core_results = vec![RuntimeVal::I32(0); result_arity];

        // The arguments are lowered, so the task's thread runs now.
        store.start_export_task(task)?;
        self.inner
            .call(store.inner_mut(), &core_args, &mut core_results)
            .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;

        // The result crosses back out, and the export's post-return
        // runs after the caller has logically observed it: the
        // lifted value is in hand, so the post-return is safe to run
        // now. Both go through the one context of the crossing.
        let lifted_result =
            self.lift_result(store, &core_results, instance, task, options.clone())?;

        Ok(lifted_result.into_iter().collect())
    }

    /// The resource tables of the instance, by table index, for the
    /// lift and lower contexts.
    fn resource_tables(&self) -> Result<Vec<Option<ResourceTableRuntime>>> {
        let state = self
            .abi_state
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
        Ok(state.resource_tables.clone())
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
        instance: InstanceId,
        task: TaskId,
        options: &BoundaryOptions,
    ) -> Result<Vec<RuntimeVal>> {
        let tables = store.tables_handle();
        let resource_tables = self.resource_tables()?;
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lower_ctx = BoundaryContext::new(
            store_ctx,
            options.clone(),
            Some(instance),
            Some(Scope::Task(task)),
            Some(tables),
            resource_tables,
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
        instance: InstanceId,
        task: TaskId,
        options: BoundaryOptions,
    ) -> Result<Option<Val>> {
        let position = AbiPosition::Result;
        let tables = store.tables_handle();
        let resource_tables = self.resource_tables()?;
        let store_ctx = store.inner_mut().as_context_mut();
        let mut lift_ctx = BoundaryContext::new(
            store_ctx,
            options,
            Some(instance),
            Some(Scope::Task(task)),
            Some(tables),
            resource_tables,
        );
        let Some(result_ty) = &self.signature.result else {
            lift_ctx.post_return(core_results)?;
            return Ok(None);
        };
        let lifted = if result_spills(&self.signature) {
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
            lift(&mut lift_ctx, ptr, result_ty, position)?
        } else {
            let mut cursor = 0usize;
            lift_from_flat_slots(
                &mut lift_ctx,
                core_results,
                &mut cursor,
                result_ty,
                position,
            )?
        };
        // The post-return's arguments are the core results: the flat
        // result slots, or the return-area pointer when the result
        // spilled to memory.
        lift_ctx.post_return(core_results)?;
        Ok(Some(lifted))
    }
}
