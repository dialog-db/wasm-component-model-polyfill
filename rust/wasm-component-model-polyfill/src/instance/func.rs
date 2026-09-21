//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use core::task::Poll;
use std::sync::{Arc, Mutex};

use wasm_runtime_layer::{AsContextMut, Val as RuntimeVal};

use crate::abi::context::BoundaryContext;
use crate::abi::flatten::{lift_from_flat_slots, lower_into_flat_slots};
use crate::abi::instance::BoundaryInstance;
use crate::abi::layout::{flat_types, params_spill, result_spills, spill_layout};
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::abi::{lift, lower};
use crate::backend::substrate_failure;
use crate::component::FunctionType;
use crate::concurrency::{
    Accessor, Driver, InstanceId, Item, ItemKind, ResultChannel, Scope, TaskId,
};
use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result, SchedulerCause,
};
use crate::executor::ir::CanonOptions;
use crate::executor::{CallbackTask, status_word};
use crate::resource::TableId;
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

/// Where the start of an asynchronous call leaves the failure that
/// belongs to the caller.
///
/// A call into an export lifted `async` resolves through the task's
/// own channel, which carries a result and not a failure. The item
/// that starts the task leaves what failed here instead: the lowering
/// of the arguments, a trap in the export's core function, or the
/// status word that function returned.
type CallFailure = Arc<Mutex<Option<Error>>>;

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
    /// An export lifted `canon lift async` with a callback does not
    /// return its result by returning. It calls `task.return`, which
    /// is what resolves the call, and its core function returns a
    /// status word that says whether the task is over or wants to be
    /// resumed. Such a task waits at its instance's entry gate before
    /// it starts, has no `post-return`, and can outlive the call: a
    /// callback that has yet to run stays in the store, and a failure
    /// it raises belongs to whichever driver's turn runs it.
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
        if args.len() != self.signature.parameters.len() {
            return Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: None,
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

        // A call into an export lifted `async` is a task that returns
        // a status word and produces its result through
        // `task.return`. The task outlives the call, so the call
        // watches the task's channel rather than a slot of its own.
        if self.options.async_ {
            return self
                .call_async(store, task, instance_id, instance, options, args)
                .await;
        }

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
        )
        .for_task(task);

        // A synchronous export of a synchronous function ignores the
        // entry gate, as the reference states: the gate applies to a
        // task whose function type is `async`, which a synchronous
        // lift can carry. The exclusive flag is the reference's
        // `not opts.async or opts.callback`, which is true here; the
        // gate reads it only for a task that does wait at it.
        store.start_export_thread(task, instance_id, self.signature.async_, true, item)?;

        Driver::new(store, Some(task), move |_store, _waker| {
            outcome.lock().ok().and_then(|mut slot| slot.take())
        })
        .await
    }

    /// Invoke the function from inside a poll of the store, with the
    /// given polyfill-typed arguments.
    ///
    /// This is the entry a host calls while something else is
    /// driving the store: the closure of the store's
    /// `run_concurrent` entry, or the body of a host `async`
    /// function. The accessor is the token those two are handed, and
    /// it is how this call reaches the store. A call made where no
    /// poll of the accessor's store is running fails with the
    /// store-not-in-poll cause, and one made from inside another
    /// reach fails with the recursive-driver cause.
    ///
    /// `T` is the host-data type of the [`Store`] the instance was
    /// created in. An accessor naming a different store fails with
    /// the store-not-in-poll cause, and an export of a different
    /// store returns [`InstantiationError::WrongStore`].
    ///
    /// The call creates the export's task and queues the start of
    /// its implicit thread behind the entry gate of its instance,
    /// exactly as [`Self::call`] does. A synchronous export's task
    /// ignores the gate and becomes ready at once. An export lifted
    /// `canon lift async` with a callback waits at the gate while
    /// another task of the same instance holds the instance
    /// exclusively, and starts when that holder releases it — on
    /// return for a synchronous task, and between events for a
    /// callback task. The returned future resolves when the task's
    /// result is set and yields the lifted result; `post-return`,
    /// for a synchronous export that declares one, runs after the
    /// result is lifted, as it does for [`Self::call`].
    ///
    /// The future is spawn-like. Dropping it cancels nothing: the
    /// task stays in the store and runs on in the next turn of any
    /// driver. The task progresses only while a driver runs turns,
    /// which in practice means while the future is awaited inside
    /// the `run_concurrent` closure. This entry is not itself a
    /// driver, and [`Self::call`], which is one, cannot be entered
    /// from that closure at all, because it takes the store by
    /// `&mut` and the closure holds only the accessor.
    ///
    /// A store that goes idle with the task unresolved leaves the
    /// `run_concurrent` entry pending rather than failing, and this
    /// future never resolves. It does not fail on idle, because the
    /// closure around it can still unblock the task with another
    /// call; a host that wants a bound on the wait bounds the whole
    /// entry with a timeout.
    ///
    /// [`Store`]: crate::Store
    pub async fn call_concurrent<T: 'static>(
        &self,
        accessor: &Accessor<T>,
        args: &[Val],
    ) -> Result<Box<[Val]>> {
        // The start runs inside one reach into the store, which is a
        // turn: it queues the task's start and runs none of it. What
        // it hands back is what the task leaves behind, which
        // outlives this future.
        let (channel, failure) = accessor.with(|store| self.start_concurrent(store, args))??;

        core::future::poll_fn(move |_context| {
            // The failure is read first. A synchronous task can
            // resolve and then fail on the borrows the guest still
            // owes, and that failure is the call's, as it is for
            // `Func::call`.
            if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
                return Poll::Ready(Err(error));
            }
            match channel.lock().ok().and_then(|mut slot| slot.take()) {
                Some(result) => Poll::Ready(Ok(result.into_iter().collect())),
                // No waker is registered here. The driver around
                // this future polls it again after every turn it
                // runs, and a turn is the only thing that carries
                // the task forward.
                None => Poll::Pending,
            }
        })
        .await
    }

    /// Start the task of one concurrent call and hand back what that
    /// call watches: the task's result channel, and the slot the
    /// start leaves a failure of the caller's in.
    ///
    /// Everything happens inside the one reach into the store, so a
    /// call whose arguments or whose export are wrong fails before
    /// anything is queued and leaves the store untouched.
    fn start_concurrent<T: 'static>(
        &self,
        store: &mut StoreContext<'_, T>,
        args: &[Val],
    ) -> Result<(ResultChannel, CallFailure)> {
        if store.id() != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        if args.len() != self.signature.parameters.len() {
            return Err(Error::from(AbiError {
                position: AbiPosition::Argument(0),
                valtype: None,
                cause: AbiCause::InvalidEncoding {
                    message: format!(
                        "expected {} arguments, got {}",
                        self.signature.parameters.len(),
                        args.len()
                    ),
                },
            }));
        }

        let (options, instance) =
            BoundaryInstance::resolve(&self.options, &self.abi_state, &store.tables_handle())?;
        let instance_id = instance.id().ok_or_else(|| {
            Error::internal("an export's lift names a component instance the plan does not hold")
        })?;

        // The caller is not on the stack when the task resolves, so
        // the task is given a channel to resolve through and the
        // call watches that rather than the record, which a
        // synchronous task's own exit takes out of the store.
        let task =
            store.create_export_task(self.signature.clone(), self.options.clone(), instance_id)?;
        let channel = store.attach_result_channel(task)?;
        let failure: CallFailure = Arc::new(Mutex::new(None));

        // The item is `'static`: it outlives this future, because
        // dropping the future cancels nothing. It therefore carries
        // its own copy of everything the call needs — the resolved
        // options among them.
        let queued = failure.clone();
        let replica = self.replica();
        let arguments = args.to_vec();

        if self.options.async_ {
            let callback = options
                .callback()
                .cloned()
                .ok_or_else(|| Error::internal("an `async` export's lift named no callback"))?;
            let table = self.handle_table()?;
            let loop_ = CallbackTask::new(task, instance_id, table, callback);
            let item = Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, T>| {
                    let started = replica
                        .start_async_task(task, &loop_, &instance, store, &arguments, &options);
                    if let Err(error) = started
                        && let Ok(mut slot) = queued.lock()
                    {
                        *slot = Some(error);
                    }
                    // What the start produced is in the task's
                    // channel and what it failed with is in the slot
                    // beside it, so the item itself has nothing left
                    // to fail with.
                    Ok(())
                },
            )
            .for_task(task);

            // The entry gate applies: the function type is `async`,
            // and a callback task needs the instance exclusively,
            // because the core code it runs between events must not
            // overlap another exclusive task of the same instance.
            store.start_export_thread(task, instance_id, true, true, item)?;
            return Ok((channel, failure));
        }

        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                let outcome = replica.run_task(task, &instance, store, &arguments, &options);
                if let Err(error) = outcome
                    && let Ok(mut slot) = queued.lock()
                {
                    *slot = Some(error);
                }
                // What the task returned went through the channel as
                // it resolved, so the item has nothing left to carry
                // and nothing left to fail with.
                Ok(())
            },
        )
        .for_task(task);

        // A synchronous export's task ignores the entry gate, as the
        // reference states: the gate applies to a task whose
        // function type is `async`. The exclusive flag is the
        // reference's `not opts.async or opts.callback`, which is
        // true here; the gate reads it only for a task that does
        // wait at it.
        store.start_export_thread(task, instance_id, false, true, item)?;
        Ok((channel, failure))
    }

    /// Invoke an export lifted `canon lift async` with a callback.
    ///
    /// The call is a task whose implicit thread waits at the entry
    /// gate of its instance while backpressure is set or another task
    /// holds the instance exclusively. Once through the gate, the
    /// item lowers the arguments, marks the task started, calls the
    /// core function, and hands the status word it returned to the
    /// callback loop. There is no post-return: the reference calls
    /// one only on the synchronous path.
    ///
    /// The call's future resolves when `task.return` sets the task's
    /// result, and it returns the lifted value. The export's core
    /// function has by then returned a status word, or it is still on
    /// the stack below `task.return`, and the driver sees the result
    /// in the same turn after it returns. The task does not end with
    /// the call: a task that yielded or waited leaves a callback item
    /// in the store, which runs in the turn of whichever driver comes
    /// next, and an error that item raises fails that driver rather
    /// than this call.
    ///
    /// A turn that finds the task at the gate or waiting, with
    /// nothing else ready, fails the call with the deadlock cause.
    async fn call_async<T: 'static>(
        &self,
        mut store: StoreContext<'_, T>,
        task: TaskId,
        instance_id: InstanceId,
        instance: BoundaryInstance,
        options: BoundaryOptions,
        args: &[Val],
    ) -> Result<Box<[Val]>> {
        let callback = options
            .callback()
            .cloned()
            .ok_or_else(|| Error::internal("an `async` export's lift named no callback"))?;
        let table = self.handle_table()?;
        let loop_ = CallbackTask::new(task, instance_id, table, callback);
        let channel: ResultChannel = store.attach_result_channel(task)?;

        let failure: CallFailure = Arc::new(Mutex::new(None));
        let queued = failure.clone();
        let replica = self.replica();
        let arguments = args.to_vec();
        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                let started =
                    replica.start_async_task(task, &loop_, &instance, store, &arguments, &options);
                if let Err(error) = started
                    && let Ok(mut slot) = queued.lock()
                {
                    *slot = Some(error);
                }
                // What the start produced is in the task's channel
                // and what it failed with is in the slot beside it,
                // so the item itself has nothing left to fail with.
                Ok(())
            },
        )
        .for_task(task);

        // The entry gate applies: the function type is `async`, and a
        // callback task needs the instance exclusively, because the
        // core code it runs between events must not overlap another
        // exclusive task of the same instance.
        store.start_export_thread(task, instance_id, true, true, item)?;

        Driver::new(store, Some(task), move |_store, _waker| {
            if let Some(error) = failure.lock().ok().and_then(|mut slot| slot.take()) {
                return Some(Err(error));
            }
            let result = channel.lock().ok().and_then(|mut slot| slot.take())?;
            Some(Ok(result.into_iter().collect()))
        })
        .await
    }

    /// Run the start of an asynchronous call's task: the task becomes
    /// the current scope, the arguments are lowered, the core
    /// function runs, the scope is popped, and the status word goes
    /// to the callback loop. A failure anywhere in that ends the task
    /// and travels out to the call.
    fn start_async_task<T: 'static>(
        &self,
        task: TaskId,
        loop_: &CallbackTask,
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        args: &[Val],
        options: &BoundaryOptions,
    ) -> Result<()> {
        store.enter_export_task(task)?;
        match self.call_async_core(task, instance, store, args, options) {
            Ok(word) => {
                store.leave_export_task(task)?;
                loop_.handle_status_word(store, word)
            }
            Err(error) => {
                store.abandon_export_task(task)?;
                Err(error)
            }
        }
    }

    /// Lower the arguments and call the core function of an
    /// asynchronous export, and report the status word it returned.
    /// A `cabi_realloc` the lowering calls is a task of its own, as
    /// the reference lifts it.
    fn call_async_core<T: 'static>(
        &self,
        task: TaskId,
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        args: &[Val],
        options: &BoundaryOptions,
    ) -> Result<i32> {
        let core_args = self.lower_args(store, args, instance, task, options)?;
        let mut core_results = vec![RuntimeVal::I32(0); 1];

        // The arguments are lowered, so the task's thread runs now.
        store.start_export_task(task)?;
        self.inner
            .call(store.runtime_mut(), &core_args, &mut core_results)
            .map_err(substrate_failure)?;
        status_word(&core_results)
    }

    /// The handle table of the component instance this export belongs
    /// to, which is where a status word resolves a waitable set
    /// index. It is read the way a built-in reads it: through the
    /// instance index the export's canon options name.
    fn handle_table(&self) -> Result<TableId> {
        let state = self
            .abi_state
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
        state
            .handle_tables
            .get(self.options.instance)
            .copied()
            .ok_or_else(|| {
                Error::internal("an export's lift names a component instance with no handle table")
            })
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
        // A host call into a sync-typed export must return before its
        // instance may block, so the flag is held for the length of
        // the call, as the enter intrinsic holds it for a synchronous
        // call between two components; the task's exit below puts it
        // back whichever way the call went. An async-typed export
        // lifted synchronously is allowed to block, and the flag
        // stays as it was: the reference lets such a callee give way
        // while its own caller waits.
        let held = if self.signature.async_ {
            Ok(())
        } else {
            store.hold_may_not_suspend(task)
        };
        let outcome = match held {
            Ok(()) => self.call_in_task(task, instance, store, args, options),
            Err(err) => Err(err),
        };
        match outcome {
            Ok(result) => {
                store.resolve_export_task(task, result.first().cloned())?;
                match store.exit_export_task(task)? {
                    Ok(()) => Ok(result),
                    // The borrow the export still owes is owed at
                    // the end of the call, not at a value the call
                    // was processing, so the failure names the
                    // export's result type when it has one and no
                    // type at all when it does not.
                    Err(count) => Err(Error::from(AbiError {
                        position: AbiPosition::Result,
                        valtype: self.signature.result.clone(),
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
            .map_err(substrate_failure)?;

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
                        valtype: Some(result_ty.clone()),
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
