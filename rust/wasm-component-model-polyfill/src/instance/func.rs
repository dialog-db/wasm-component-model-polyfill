//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{AsContextMut, Val as RuntimeVal};

use crate::abi::context::BoundaryContext;
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{flat_types, params_spill, result_spills, spill_layout};
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift, lower};
use crate::component::FunctionType;
use crate::concurrency::{Driver, Item, ItemKind, Scope, TaskId};
use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result, SchedulerCause,
};
use crate::executor::ir::CanonOptions;
use crate::store::{Store, StoreContext, StoreId};
use crate::types::{PrimitiveType, ValueType};
use crate::value::Val;

/// Where the queued item of one call leaves what the call produced.
///
/// The slot is what the call's driver watches: the item fills it
/// when the export's task resolves or fails, and the driver takes
/// the value out. Both sides hold it, because the item outlives the
/// future when the future is dropped.
type CallOutcome = Arc<Mutex<Option<Result<Box<[Val]>>>>>;

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
    /// The call is a driver of the store's cooperative scheduler: it
    /// creates a task for the export, queues the start of the task's
    /// implicit thread, and polls the scheduler in turns until the
    /// task resolves. A synchronous export's task resolves when its
    /// core function returns. Work the task leaves behind stays in
    /// the store, and the future does not wait for it; dropping the
    /// future cancels nothing, and the task runs in the next turn of
    /// any driver.
    ///
    /// Entering the call while another driver of the same store is
    /// inside a turn fails with the recursive-driver cause, and a
    /// turn that goes idle with the task unresolved fails with the
    /// deadlock cause, or with the cannot-block cause when the task
    /// must not block.
    pub async fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
        let mut store = store.context();
        if store.id() != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        // A call into an export lifted `async` is a task that
        // returns a status word and produces its result through
        // `task.return`. The runtime that reads the word is not
        // built yet, so the call is refused here rather than lifting
        // the word as though it were the export's result.
        if self.options.async_ {
            return Err(Error::unsupported("host calls into an asynchronous export"));
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

        // A driver entered while another driver of the same store is
        // inside a turn fails before it has created a task or queued
        // anything, so a refused call leaves the store untouched.
        if store.turn_in_flight() {
            return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
        }

        // The canon options of the export's lift and the instance
        // they name, read out of the instance's runtime state once
        // for the whole call. Each crossing of the call builds its
        // boundary context from the two, and the instance is where
        // the handle tables of the crossing come from.
        let (options, instance) =
            BoundaryInstance::resolve(&self.options, &self.abi_state, &store.tables_handle())?;
        let instance_id = instance.id().ok_or_else(|| {
            Error::internal("an export's lift names a component instance the plan does not hold")
        })?;

        // A call from the host into the guest is a task. The record
        // is created now, so that it exists whether or not a turn
        // ever runs the item that starts it; the task goes on the
        // stack of current scopes only when its thread runs. Borrows
        // the host lowers in are owed to it and must be dropped by
        // the guest before the call ends; borrows the guest lifts out
        // in results lend to it until the call ends.
        let task =
            store.create_export_task(self.signature.clone(), self.options.clone(), instance_id)?;

        // The item is `'static`: it outlives this future, because
        // dropping the future cancels nothing. It therefore carries
        // its own copy of everything the call needs — the resolved
        // options among them — and leaves what the call produced in
        // a slot both sides hold.
        let outcome: CallOutcome = Arc::new(Mutex::new(None));
        let queued = outcome.clone();
        let replica = self.replica();
        let arguments = args.to_vec();
        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                let result = replica.run_task(task, &instance, store, &arguments, &options);
                if let Ok(mut slot) = queued.lock() {
                    *slot = Some(result);
                }
                // The failure of the call is the caller's, and it is in
                // the slot the driver reads, so the item itself has
                // nothing left to fail with.
                Ok(())
            },
        );

        // A synchronous export's task ignores the entry gate, as the
        // reference states: the gate applies to a task whose function
        // type is `async`, and a call into such an export is refused
        // above. The exclusive flag is the reference's
        // `not opts.async or opts.callback`, which is true here; the
        // gate reads it only for a task that does wait at it.
        store.start_export_thread(task, instance_id, false, true, item)?;

        Driver::new(store, Some(task), move |_store, _waker| {
            outcome.lock().ok().and_then(|mut slot| slot.take())
        })
        .await
    }

    /// A `'static` copy of this handle, for the item that runs the
    /// call. Every field is a name, a handle, or a description, so
    /// the copy drives the same export as the original.
    fn replica(&self) -> Self {
        Self {
            name: self.name.clone(),
            inner: self.inner.clone(),
            signature: self.signature.clone(),
            options: self.options.clone(),
            abi_state: self.abi_state.clone(),
            store_id: self.store_id,
        }
    }

    /// Run the export's task to its resolution: push it as the
    /// current scope, drive the canonical-ABI round-trip, resolve the
    /// task with what the export returned, and pop the scope. The
    /// pop ends the task's implicit thread whichever way the call
    /// went, so the instance it held exclusively, if it held one,
    /// goes back before this returns.
    fn run_task<T: 'static>(
        &self,
        task: TaskId,
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        args: &[Val],
        options: &BoundaryOptions,
    ) -> Result<Box<[Val]>> {
        store.enter_export_task(task)?;
        let outcome = self.call_in_task(task, instance, store, args, options);
        match outcome {
            Ok(result) => {
                store.resolve_export_task(task, result.first().cloned())?;
                match store.exit_export_task(task)? {
                    Ok(()) => Ok(result),
                    Err(count) => Err(Error::from(AbiError {
                        position: AbiPosition::Result,
                        valtype: ValueType::Primitive(PrimitiveType::Bool),
                        cause: AbiCause::OutstandingBorrows {
                            count: count as usize,
                        },
                    })),
                }
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
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        args: &[Val],
        options: &BoundaryOptions,
    ) -> Result<Box<[Val]>> {
        let core_args = self.lower_args(store, args, instance, task, options)?;
        let result_arity = self.core_result_arity();
        let mut core_results = vec![RuntimeVal::I32(0); result_arity];

        // The arguments are lowered, so the task's thread runs now.
        store.start_export_task(task)?;
        self.inner
            .call(store.runtime_mut(), &core_args, &mut core_results)
            .map_err(|err| Error::from(InstantiationError::SubstrateFailure(err)))?;

        // The result crosses back out, and the export's post-return
        // runs after the caller has logically observed it: the
        // lifted value is in hand, so the post-return is safe to run
        // now. Both go through the one context of the crossing.
        let lifted_result =
            self.lift_result(store, &core_results, instance, task, options.clone())?;

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
        store: &mut StoreContext<'_, T>,
        args: &[Val],
        instance: &BoundaryInstance,
        task: TaskId,
        options: &BoundaryOptions,
    ) -> Result<Vec<RuntimeVal>> {
        let store_ctx = store.runtime_mut().as_context_mut();
        let mut lower_ctx = BoundaryContext::new(
            store_ctx,
            options.clone(),
            instance.clone(),
            Some(Scope::Task(task)),
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
        store: &mut StoreContext<'_, T>,
        core_results: &[RuntimeVal],
        instance: &BoundaryInstance,
        task: TaskId,
        options: BoundaryOptions,
    ) -> Result<Option<Val>> {
        let position = AbiPosition::Result;
        let store_ctx = store.runtime_mut().as_context_mut();
        let mut lift_ctx = BoundaryContext::new(
            store_ctx,
            options,
            instance.clone(),
            Some(Scope::Task(task)),
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
