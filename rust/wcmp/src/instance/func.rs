// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A handle to one exported function of an [`Instance`].
//!
//! [`Instance`]: super::Instance

use core::task::Poll;
use std::sync::{Arc, Mutex};

use crate::abi::context::BoundaryContext;
use crate::abi::instance::BoundaryInstance;
use crate::abi::options::BoundaryOptions;
use crate::abi::runtime_state::AbiRuntimeState;
use crate::component::FunctionType;
use crate::concurrency::{
    Accessor, Driver, InstanceId, Item, ItemKind, ResultChannel, Scope, TaskId, WakeSlot,
};
use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, InstantiationError, Result, SchedulerCause, TaskCause,
};
use crate::executor::ir::CanonOptions;
use crate::executor::{AsyncLift, CallbackTask};
use crate::instance::ExportedFunction;
use crate::internal::{ErrorInternal, FuncInternal, FuncParts};
use crate::resource::TableId;
use crate::runtime_layer::{AsContextMut, Val as RuntimeVal};
use crate::store::{Store, StoreContext, StoreId};
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::value::Val;

use super::call_values::CallValues;

/// Where the queued item of one call leaves what the call produced.
///
/// The slot is what the call's driver watches: the item fills it
/// when the export's task resolves, and the driver takes the value
/// out. Both sides hold it, because the item outlives the future
/// when the future is dropped. A trap does not go here: it ends the
/// turn that met it.
type CallOutcome<O> = Arc<Mutex<Option<O>>>;

/// Where a concurrent call takes its result from.
enum Delivery<O> {
    /// A synchronous export's result, in the shape the call carries:
    /// the item that ran the task leaves it here once the task has
    /// ended cleanly.
    Returned(WakeSlot<O>),
    /// The channel an export lifted `async` resolves its task through
    /// when it calls `task.return`.
    Resolved(ResultChannel),
}

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
    /// The export this handle calls: its leaf name, the runtime-layer
    /// core function it resolves to, its component-level signature
    /// with the canonical-ABI layout, and the canon options its lift
    /// declared. The instance and every other handle for the export
    /// share it, so neither a lookup nor a call copies any of it.
    export: Arc<ExportedFunction>,
    /// The instance's canonical-ABI runtime state. Shared with
    /// every host trampoline the same instance carries; the lock
    /// is taken briefly at the boundaries.
    abi_state: Arc<Mutex<AbiRuntimeState>>,
    /// The identity of the [`Store`] the owning instance was
    /// created in. A call made through any other store is rejected
    /// before it reaches the runtime layer.
    store_id: StoreId,
}

impl From<FuncParts> for Func {
    fn from(parts: FuncParts) -> Self {
        Self {
            export: parts.export,
            abi_state: parts.abi_state,
            store_id: parts.store_id,
        }
    }
}

impl FuncInternal for Func {
    fn name(&self) -> &str {
        &self.export.name
    }

    fn signature(&self) -> &FunctionType {
        self.export.signature.ty()
    }

    fn options(&self) -> &CanonOptions {
        &self.export.options
    }

    fn abi_state(&self) -> &Arc<Mutex<AbiRuntimeState>> {
        &self.abi_state
    }
}

impl Func {
    /// The component-level signature of the export: its parameters
    /// and its result, as the component declares them.
    pub fn ty(&self) -> &FunctionType {
        self.export.signature.ty()
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
    /// An export lifted `canon lift async` with no callback is the
    /// stackful form, which an engine accepts only with
    /// `EngineConfig::wasm_component_model_async_stackful` on. Its
    /// core function runs as the task's implicit thread, returns
    /// nothing, and calls `task.return` to resolve the call. It does
    /// not take its instance exclusively. A core function that returns
    /// without calling `task.return` fails the call with the no-result
    /// cause. It runs on the real stack, so a block inside it waits in
    /// a nested turn, and a block that only a frame below it can
    /// release fails with the stack-switch cause.
    ///
    /// Entering the call while another driver of the same store is
    /// inside a turn fails with the recursive-driver cause, and a
    /// turn that goes idle with the task unresolved fails with the
    /// deadlock cause, or with the cannot-block cause when the task
    /// must not block.
    ///
    /// # A trap poisons the store
    ///
    /// A trap poisons the store, and a poisoned store runs no more
    /// guest code: this call, [`Self::call_concurrent`], an
    /// instantiation, and the release of a resource a guest defines
    /// all fail with the cannot-enter cause, [`TaskCause::CannotEnter`],
    /// before they change anything. A trap is a failure of the
    /// export's core code, or of a built-in, a host function, a lift,
    /// or a lower that code reached, a host `async` function whose
    /// future fails, a deadlock, or a [`Val`] of the
    /// wrong type for a parameter, which this call finds as it lowers
    /// the arguments. An arity mismatch, a call through another
    /// store, and the recursive-driver cause do not poison the store,
    /// because each is refused before anything changes.
    ///
    /// [`TaskCause::CannotEnter`]: crate::TaskCause::CannotEnter
    ///
    /// # Where a trap surfaces
    ///
    /// The first trap ends the driver that is polling the store, with
    /// that trap, and poisons the store in the same step. While this
    /// call runs turns, this call is that driver, whichever task the
    /// trap belongs to:
    ///
    /// - A trap in this call's own task ends this call.
    /// - A trap in work that another task left after it resolved — a
    ///   callback, or a thread that outlives the task's host call —
    ///   ends this call when one of its turns runs that work.
    /// - A host `async` function whose future fails is a trap of the
    ///   guest task that called it, and ends this call when one of its
    ///   turns polls that future.
    ///
    /// The trap is never held for a caller that already has its
    /// result. A task that resolved this call and then traps in work
    /// it left in the store fails whichever driver's turn runs that
    /// work, which is a later driver when this call has already
    /// returned: that driver reports the trap, and this call never
    /// learns of it. Every driver after the trap fails with the
    /// cannot-enter cause. The same rule makes [`Store::run_concurrent`]
    /// report a trap that its own turns meet.
    ///
    /// [`Store::run_concurrent`]: crate::Store::run_concurrent
    pub async fn call<T: 'static>(&self, store: &mut Store<T>, args: &[Val]) -> Result<Box<[Val]>> {
        self.call_values(store, args.to_vec()).await
    }

    /// The body of [`Self::call`], for whatever the call carries: the
    /// untyped call's [`Val`]s, or a typed call's Rust values.
    #[doc(hidden)]
    pub async fn call_values<T: 'static, C: CallValues>(
        &self,
        store: &mut Store<T>,
        values: C,
    ) -> Result<C::Output> {
        let mut store = store.internal().context();
        if store.internal().id() != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        values.check_arity(self.ty())?;

        // A driver entered while another driver of the same store is
        // inside a turn fails before it has created a task or queued
        // anything, so a refused call leaves the store untouched.
        if store.internal().turn_in_flight() {
            return Err(Error::Scheduler(SchedulerCause::RecursiveDriver));
        }

        // A store a trap poisoned runs no more guest code, and the
        // refusal, like the ones above, comes before the call has
        // changed anything.
        store.internal().enter_guest()?;

        // The canon options of the export's lift and the instance
        // they name, read out of the instance's runtime state once
        // for the whole call. Each crossing of the call builds its
        // boundary context from the two, and the instance is where
        // the handle tables of the crossing come from.
        let (options, instance) = BoundaryInstance::resolve(
            &self.export.options,
            &self.abi_state,
            &store.internal().tables_handle(),
        )?;
        let instance_id = instance.id().ok_or_else(|| {
            Error::internal("an export's lift names a component instance the plan does not hold")
        })?;

        // A call from the host into the guest is a task. The record
        // is created now, so that it exists whether or not a turn
        // ever runs the item that starts it; the task goes on the
        // stack of current scopes only when its thread runs. Borrows
        // the host lowers in are owed to it and must be dropped by
        // the guest before the call ends; handles the host lends for
        // the call go on the task and come back when the task
        // resolves, which is when this call yields its result. A
        // callback export that resolves and keeps running therefore
        // holds no host handle past its `task.return`.
        let task = store.internal().create_export_task(
            Arc::clone(&self.export.signature),
            Arc::clone(&self.export.options),
            instance_id,
        )?;

        // A call into an export lifted `async` is a task that produces
        // its result through `task.return`. The task outlives the call, so the call
        // watches the task's channel rather than a slot of its own.
        if self.export.options.async_ {
            return self
                .call_async(store, task, instance_id, instance, options, values)
                .await;
        }

        // The item is `'static`: it outlives this future, because
        // dropping the future cancels nothing. It therefore carries
        // its own copy of everything the call needs — the resolved
        // options among them — and leaves what the call produced in
        // a slot both sides hold.
        let outcome: CallOutcome<C::Output> = Arc::new(Mutex::new(None));
        let queued = outcome.clone();
        let replica = self.replica();
        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                // A trap of the task fails the item, which ends the
                // turn that ran it, so the driver that is polling
                // reports it even when this call's future is gone.
                replica.run_task(
                    task,
                    &instance,
                    store,
                    values,
                    &options,
                    SyncDelivery::Outcome(queued),
                )
            },
        )
        .for_task(task);

        // A synchronous export of a synchronous function ignores the
        // entry gate, as the reference states: the gate applies to a
        // task whose function type is `async`, which a synchronous
        // lift can carry. The exclusive flag is the reference's
        // `not opts.async or opts.callback`, which is true here; the
        // gate reads it only for a task that does wait at it.
        store
            .internal()
            .start_export_thread(task, instance_id, self.ty().async_, true, item)?;

        Driver::run(store, Some(task), move |_store, _waker| {
            match outcome.lock() {
                Ok(mut slot) => slot.take().map(Ok),
                Err(_) => Some(Err(Error::internal("a call's outcome slot is poisoned"))),
            }
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
    /// callback task. A stackful export waits at the gate only while
    /// backpressure is set or an earlier task is waiting there. The returned future resolves when the task's
    /// result is set and yields the lifted result; `post-return`,
    /// for a synchronous export that declares one, runs after the
    /// result is lifted, as it does for [`Self::call`].
    ///
    /// The future is spawn-like. Dropping it cancels nothing: the
    /// task stays in the store and runs on in the next turn of any
    /// driver, unless a trap poisons the store first and discards
    /// the task's queued work. The task progresses only while a
    /// driver runs turns, which in practice means while the future is
    /// awaited inside the `run_concurrent` closure. This entry is not itself a
    /// driver, and [`Self::call`], which is one, cannot be entered
    /// from that closure at all, because it takes the store by
    /// `&mut` and the closure holds only the accessor.
    ///
    /// The future registers the waker it is polled with, and the
    /// turn that resolves or fails the task wakes it. Several of
    /// these futures can therefore be awaited together through a
    /// combinator that polls a child again only after that child's
    /// waker fires, which is what the host combinators of the
    /// `FuturesUnordered` shape do.
    ///
    /// A store that goes idle with the task unresolved leaves the
    /// `run_concurrent` entry pending rather than failing, and this
    /// future never resolves. It does not fail on idle, because the
    /// closure around it can still unblock the task with another
    /// call; a host that wants a bound on the wait bounds the whole
    /// entry with a timeout.
    ///
    /// A store a trap poisoned refuses the call with the cannot-enter
    /// cause before it creates a task, as [`Self::call`] states.
    ///
    /// A trap of the task this call started does not come back
    /// through this future. The trap ends the driver that is polling,
    /// which is the `run_concurrent` entry around the closure: the
    /// entry returns the trap, and the closure is dropped with this
    /// future and every other call future inside it. A trap that
    /// poisoned the store in some other way, in a destructor the
    /// closure released through its accessor, leaves this future with
    /// nothing to wait for, and it fails with the cannot-enter cause
    /// the next time it is polled.
    ///
    /// [`Store`]: crate::Store
    pub async fn call_concurrent<T: 'static>(
        &self,
        accessor: &Accessor<T>,
        args: &[Val],
    ) -> Result<Box<[Val]>> {
        self.call_concurrent_values(accessor, args.to_vec()).await
    }

    /// The body of [`Self::call_concurrent`], for whatever the call
    /// carries: the untyped call's [`Val`]s, or a typed call's Rust
    /// values.
    #[doc(hidden)]
    pub async fn call_concurrent_values<T: 'static, C: CallValues>(
        &self,
        accessor: &Accessor<T>,
        values: C,
    ) -> Result<C::Output> {
        // The start runs inside one reach into the store, which is a
        // turn: it queues the task's start and runs none of it. What
        // it hands back is what the task leaves behind, which
        // outlives this future.
        let delivery = accessor.with(|store| self.start_concurrent(store, values))??;

        core::future::poll_fn(move |context| {
            let waker = context.waker();
            let delivered = match &delivery {
                Delivery::Returned(slot) => slot.take_or_wait(waker).map(|kept| kept.map(Ok)),
                Delivery::Resolved(channel) => channel
                    .take_or_wait(waker)
                    .map(|kept| kept.map(C::from_resolution)),
            };
            match delivered {
                Ok(Some(result)) => Poll::Ready(result),
                Err(error) => Poll::Ready(Err(error)),
                // A store a trap poisoned discarded the task's work,
                // so nothing will fill the slot. A trap in a turn
                // ends the entry around this future before it is
                // polled again, so what finds the store poisoned here
                // is a trap the closure met through its accessor.
                Ok(None) if poisoned(accessor) => {
                    Poll::Ready(Err(Error::Task(TaskCause::CannotEnter)))
                }
                // The waker is left in the slot, and whatever fills it
                // wakes it. A turn is the only thing that carries the
                // task forward, and the driver that runs turns polls
                // this future again after each one only when it owns
                // it directly. A host combinator that owns it instead
                // — anything of the `FuturesUnordered` shape — polls
                // it again only after the wake, so the wake is what
                // the call resolves by.
                Ok(None) => Poll::Pending,
            }
        })
        .await
    }

    /// Start the task of one concurrent call and hand back what that
    /// call watches: where its result arrives. A failure of the task
    /// is a trap, which ends the turn that met it rather than coming
    /// back to the call.
    ///
    /// Everything happens inside the one reach into the store, so a
    /// call whose arguments or whose export are wrong fails before
    /// anything is queued and leaves the store untouched.
    fn start_concurrent<T: 'static, C: CallValues>(
        &self,
        store: &mut StoreContext<'_, T>,
        values: C,
    ) -> Result<Delivery<C::Output>> {
        if store.internal().id() != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        values.check_arity(self.ty())?;
        store.internal().enter_guest()?;

        let (options, instance) = BoundaryInstance::resolve(
            &self.export.options,
            &self.abi_state,
            &store.internal().tables_handle(),
        )?;
        let instance_id = instance.id().ok_or_else(|| {
            Error::internal("an export's lift names a component instance the plan does not hold")
        })?;

        let task = store.internal().create_export_task(
            Arc::clone(&self.export.signature),
            Arc::clone(&self.export.options),
            instance_id,
        )?;

        // The item is `'static`: it outlives this future, because
        // dropping the future cancels nothing. It therefore carries
        // its own copy of everything the call needs — the resolved
        // options among them.
        let replica = self.replica();

        if self.export.options.async_ {
            // The caller is not on the stack when `task.return`
            // resolves the task, so the task is given a channel to
            // resolve through and the call watches that rather than
            // the record.
            let channel = store.internal().attach_result_channel(task)?;
            let lift = self.async_lift(task, instance_id, &options)?;
            let needs_exclusive = lift.needs_exclusive();
            let item = Item::new(
                ItemKind::TaskStart,
                move |store: &mut StoreContext<'_, T>| {
                    // What the start produces is in the task's
                    // channel, and a trap of the task fails the item,
                    // which ends the turn that ran it.
                    replica.start_async_task(task, lift, &instance, store, values, &options)
                },
            )
            .for_task(task);

            // The entry gate applies: the function type is `async`.
            // A callback task needs the instance exclusively, because
            // the core code it runs between events must not overlap
            // another exclusive task of the same instance. A stackful
            // task does not.
            store
                .internal()
                .start_export_thread(task, instance_id, true, needs_exclusive, item)?;
            return Ok(Delivery::Resolved(channel));
        }

        // A synchronous export's result is the one its lift produced,
        // in whatever shape the call carries, and the item leaves it
        // for the call once the task has ended cleanly. The task
        // resolves while the item runs, so the result is in the slot
        // before the turn that ran the item is over.
        let returned: WakeSlot<C::Output> = WakeSlot::new();
        let delivered = returned.clone();
        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                // What the task returns goes in the slot, and a trap
                // of the task fails the item, which ends the turn
                // that ran it.
                replica.run_task(
                    task,
                    &instance,
                    store,
                    values,
                    &options,
                    SyncDelivery::Returned(delivered),
                )
            },
        )
        .for_task(task);

        // A synchronous export's task ignores the entry gate, as the
        // reference states: the gate applies to a task whose
        // function type is `async`. The exclusive flag is the
        // reference's `not opts.async or opts.callback`, which is
        // true here; the gate reads it only for a task that does
        // wait at it.
        store
            .internal()
            .start_export_thread(task, instance_id, false, true, item)?;
        Ok(Delivery::Returned(returned))
    }

    /// Invoke an export lifted `canon lift async`, with a callback or
    /// stackful.
    ///
    /// The call is a task whose implicit thread waits at the entry
    /// gate of its instance while backpressure is set, or, for a
    /// callback export, while another task holds the instance
    /// exclusively. Once through the gate, the item lowers the
    /// arguments, marks the task started, and calls the core
    /// function. A callback export's core function returns a status
    /// word, which goes to the callback loop. A stackful export's
    /// core function runs as the task's implicit thread and returns
    /// nothing, and its return ends the thread. There is no
    /// post-return: the reference calls one only on the synchronous
    /// path.
    ///
    /// The call's future resolves when `task.return` sets the task's
    /// result, and it returns the lifted value. The export's core
    /// function has by then returned, or it is still on the stack
    /// below `task.return`, and the driver sees the result in the
    /// same turn after it returns. A callback task does not end with
    /// the call: a task that yielded or waited leaves a callback item
    /// in the store, which runs in the turn of whichever driver comes
    /// next, and an error that item raises fails that driver rather
    /// than this call. A stackful core function that returns without
    /// `task.return` fails the call with the no-result cause.
    ///
    /// A turn that finds the task at the gate or waiting, with
    /// nothing else ready, fails the call with the deadlock cause.
    async fn call_async<T: 'static, C: CallValues>(
        &self,
        mut store: StoreContext<'_, T>,
        task: TaskId,
        instance_id: InstanceId,
        instance: BoundaryInstance,
        options: BoundaryOptions,
        values: C,
    ) -> Result<C::Output> {
        let lift = self.async_lift(task, instance_id, &options)?;
        let needs_exclusive = lift.needs_exclusive();
        let channel: ResultChannel = store.internal().attach_result_channel(task)?;

        let replica = self.replica();
        let item = Item::new(
            ItemKind::TaskStart,
            move |store: &mut StoreContext<'_, T>| {
                // What the start produces is in the task's channel, and
                // a trap of the task fails the item, which ends the
                // turn that ran it and this call with it.
                replica.start_async_task(task, lift, &instance, store, values, &options)
            },
        )
        .for_task(task);

        // The entry gate applies: the function type is `async`. A
        // callback task needs the instance exclusively, because the
        // core code it runs between events must not overlap another
        // exclusive task of the same instance. A stackful task does
        // not.
        store
            .internal()
            .start_export_thread(task, instance_id, true, needs_exclusive, item)?;

        Driver::run(store, Some(task), move |_store, waker| {
            match channel.take_or_wait(waker) {
                Ok(result) => result.map(C::from_resolution),
                Err(error) => Some(Err(error)),
            }
        })
        .await
    }

    /// The form of this export's asynchronous lift for the call that
    /// is `task`: the callback loop when the lift names a callback,
    /// and the stackful form when it names none.
    fn async_lift(
        &self,
        task: TaskId,
        instance_id: InstanceId,
        options: &BoundaryOptions,
    ) -> Result<AsyncLift> {
        if self.export.options.callback.is_none() {
            return Ok(AsyncLift::Stackful(task));
        }
        let callback = options.callback().cloned().ok_or_else(|| {
            Error::internal(
                "an `async` export's lift names a callback the instance did not extract",
            )
        })?;
        let table = self.handle_table()?;
        Ok(AsyncLift::Callback(CallbackTask::new(
            task,
            instance_id,
            table,
            callback,
        )))
    }

    /// Run the start of an asynchronous call's task: the task becomes
    /// the current scope, the arguments are lowered, the core
    /// function runs as the task's implicit thread, the scope is
    /// popped, and what the core function returned goes to `lift`. A
    /// failure anywhere in that ends the task, poisons the store, and
    /// fails the item, which ends the turn that ran it.
    ///
    /// The core function is the thread's entry, so it starts through
    /// the store's provider when there is one, and the part after it
    /// runs when the entry finishes, which is after the thread
    /// suspended and resumed when it blocked on the way. A `cabi_realloc`
    /// the lowering calls is a task of its own, as the reference lifts
    /// it, and runs where it stands.
    ///
    fn start_async_task<T: 'static, C: CallValues>(
        &self,
        task: TaskId,
        lift: AsyncLift,
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        values: C,
        options: &BoundaryOptions,
    ) -> Result<()> {
        let base = store.internal().scope_depth()?;
        if let Err(error) = store.internal().enter_export_task(task) {
            return trap(store, error);
        }
        let started = self
            .lower_args(store, values, instance, task, options)
            .and_then(|core_args| {
                // The arguments are lowered, so the task's thread
                // runs now.
                store.internal().start_export_task(task)?;
                Ok(core_args)
            });
        let core_args = match started {
            Ok(core_args) => core_args,
            Err(error) => {
                let error = abandoned(store, task, error);
                return trap(store, error);
            }
        };
        let thread = store.internal().implicit_thread(task)?;
        let slots = vec![RuntimeVal::I32(0); lift.result_count()];
        let finish = move |store: &mut StoreContext<'_, T>, called: Result<Vec<RuntimeVal>>| {
            let outcome = match called {
                Ok(core_results) => store
                    .internal()
                    .leave_export_task(task)
                    .and_then(|()| lift.returned(store, &core_results)),
                Err(error) => Err(abandoned(store, task, error)),
            };
            match outcome {
                Ok(()) => Ok(()),
                Err(error) => trap(store, error),
            }
        };
        store.internal().run_thread_entry(
            thread,
            base,
            &self.export.func,
            &core_args,
            slots,
            finish,
        )
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
            .get(self.export.options.instance)
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
            export: Arc::clone(&self.export),
            abi_state: self.abi_state.clone(),
            store_id: self.store_id,
        }
    }

    /// Run the export's task to its resolution: push it as the
    /// current scope, drive the canonical-ABI round-trip, resolve the
    /// task with what the export returned, pop the scope, and hand
    /// what the call produced to `delivery`. The pop ends the task's
    /// implicit thread whichever way the call went, so the instance it
    /// held exclusively, if it held one, goes back.
    ///
    /// The core function is the thread's entry, so it starts through
    /// the store's provider when there is one, and the crossing of
    /// its result runs when the entry finishes, which is after the
    /// thread suspended and resumed when it blocked on the way.
    ///
    /// A trap of the task fails this, which ends the turn that ran
    /// it, so the driver that is polling reports it.
    fn run_task<T: 'static, C: CallValues>(
        &self,
        task: TaskId,
        instance: &BoundaryInstance,
        store: &mut StoreContext<'_, T>,
        values: C,
        options: &BoundaryOptions,
        delivery: SyncDelivery<C::Output>,
    ) -> Result<()> {
        let base = store.internal().scope_depth()?;
        if let Err(error) = store.internal().enter_export_task(task) {
            return delivery.deliver(store, Err(error));
        }
        // A host call into a sync-typed export must return before its
        // instance may block, so the flag is held for the length of
        // the call, as the enter intrinsic holds it for a synchronous
        // call between two components; the task's exit below puts it
        // back whichever way the call went. An async-typed export
        // lifted synchronously is allowed to block, and the flag
        // stays as it was: the reference lets such a callee give way
        // while its own caller waits.
        let held = if self.ty().async_ {
            Ok(())
        } else {
            store.internal().hold_may_not_suspend(task)
        };
        let started = held
            .and_then(|()| self.lower_args(store, values, instance, task, options))
            .and_then(|core_args| {
                // The arguments are lowered, so the task's thread
                // runs now.
                store.internal().start_export_task(task)?;
                Ok(core_args)
            });
        let core_args = match started {
            Ok(core_args) => core_args,
            Err(error) => {
                let error = abandoned(store, task, error);
                return delivery.deliver(store, Err(error));
            }
        };
        let thread = store.internal().implicit_thread(task)?;
        let slots = vec![RuntimeVal::I32(0); self.export.signature.core_result_arity()];
        let replica = self.replica();
        let instance = instance.clone();
        let options = options.clone();
        let finish = move |store: &mut StoreContext<'_, T>, called: Result<Vec<RuntimeVal>>| {
            // The result crosses back out, and the export's
            // post-return runs after the caller has logically
            // observed it: the lifted value is in hand, so the
            // post-return is safe to run now. Both go through the one
            // context of the crossing.
            let lifted = called.and_then(|core_results| {
                replica.lift_result::<T, C>(store, &core_results, &instance, task, options)
            });
            let result = match lifted {
                Ok(result) => replica.end_task::<T, C>(store, task, result),
                Err(error) => Err(abandoned(store, task, error)),
            };
            delivery.deliver(store, result)
        };
        store.internal().run_thread_entry(
            thread,
            base,
            &self.export.func,
            &core_args,
            slots,
            finish,
        )
    }

    /// End the task of a synchronous export whose result the call
    /// lifted: resolve it with the result, and pop it. A borrow the
    /// export still owes fails the call.
    fn end_task<T: 'static, C: CallValues>(
        &self,
        store: &mut StoreContext<'_, T>,
        task: TaskId,
        result: C::Output,
    ) -> Result<C::Output> {
        store
            .internal()
            .resolve_export_task(task, C::resolution(&result))?;
        match store.internal().exit_export_task(task)? {
            Ok(()) => Ok(result),
            // The borrow the export still owes is owed at the end of
            // the call, not at a value the call was processing, so the
            // failure names the export's result type when it has one
            // and no type at all when it does not.
            Err(count) => Err(Error::from(AbiError {
                position: AbiPosition::Result,
                valtype: self.ty().result.clone(),
                cause: AbiCause::OutstandingBorrows {
                    count: count as usize,
                },
            })),
        }
    }

    fn lower_args<T: 'static, C: CallValues>(
        &self,
        store: &mut StoreContext<'_, T>,
        values: C,
        instance: &BoundaryInstance,
        task: TaskId,
        options: &BoundaryOptions,
    ) -> Result<Vec<RuntimeVal>> {
        let store_ctx = store.internal().runtime_mut().as_context_mut();
        let mut lower_ctx = BoundaryContext::new(
            store_ctx,
            options.clone(),
            instance.clone(),
            Some(Scope::Task(task)),
        );
        values.lower(&mut lower_ctx, &self.export.signature)
    }

    fn lift_result<T: 'static, C: CallValues>(
        &self,
        store: &mut StoreContext<'_, T>,
        core_results: &[RuntimeVal],
        instance: &BoundaryInstance,
        task: TaskId,
        options: BoundaryOptions,
    ) -> Result<C::Output> {
        let store_ctx = store.internal().runtime_mut().as_context_mut();
        let mut lift_ctx = BoundaryContext::new(
            store_ctx,
            options,
            instance.clone(),
            Some(Scope::Task(task)),
        );
        let lifted = C::lift(&mut lift_ctx, core_results, self.ty())?;
        // The post-return's arguments are the core results: the flat
        // result slots, or the return-area pointer when the result
        // spilled to memory.
        lift_ctx.post_return(core_results)?;
        Ok(lifted)
    }
}

/// End `task` on its failure path, which is what a call whose
/// arguments, core function, or result failed comes to, and answer
/// the failure the call reports: `error`, or the failure the end
/// itself met.
fn abandoned<T: 'static>(store: &mut StoreContext<'_, T>, task: TaskId, error: Error) -> Error {
    match store.internal().abandon_export_task(task) {
        Ok(()) => error,
        Err(failure) => failure,
    }
}

/// Fail an asynchronous export's task with `error`. The failure of
/// the task is a trap: its lowering, its core code, a built-in that
/// code called, or what it returned failed. A trap poisons the store,
/// so no guest code runs again, and it ends the turn that met it, so
/// the driver that is polling reports it.
fn trap<T: 'static>(store: &mut StoreContext<'_, T>, error: Error) -> Result<()> {
    store.internal().poison();
    Err(error)
}

/// Whether a trap poisoned the store `accessor` reaches, read from
/// inside a poll of it. A reach that fails answers no: the poll that
/// is not lending the store is not one this reads for.
fn poisoned<T: 'static>(accessor: &Accessor<T>) -> bool {
    matches!(
        accessor.with(|store| store.internal().enter_guest()),
        Ok(Err(_))
    )
}

/// Where a synchronous export's task leaves the result of its call.
/// A trap goes to neither: it ends the turn instead.
enum SyncDelivery<O> {
    /// The slot the driver of one call watches.
    Outcome(CallOutcome<O>),
    /// The result slot of a concurrent call.
    Returned(WakeSlot<O>),
}

impl<O> SyncDelivery<O> {
    /// Leave the call's result where its caller watches. A failure is
    /// a trap of the call's task — its lowering, its core code, a
    /// built-in that code called, its lift, or the borrows it still
    /// owed — and poisons the store. It fails this, which ends the
    /// turn, so the driver that is polling reports it. That driver is
    /// the call itself while its future is polled, and a later driver
    /// when the future was dropped before the task ran: a slot that no
    /// driver watches would lose the trap.
    fn deliver<T: 'static>(self, store: &mut StoreContext<'_, T>, result: Result<O>) -> Result<()> {
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                store.internal().poison();
                return Err(error);
            }
        };
        match self {
            Self::Outcome(slot) => {
                *slot
                    .lock()
                    .map_err(|_| Error::internal("a call's outcome slot is poisoned"))? =
                    Some(value);
                Ok(())
            }
            Self::Returned(slot) => slot.fill(value),
        }
    }
}
