// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Everything a store carries, the host's data and the polyfill's
//! own.

use core::task::Waker;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::concurrency::{InstanceId, Scheduler, StoreProvider, TurnGuard};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::internal::ErrorInternal;
use crate::resource::{
    HandleLookupError, HandleTables, ResourceHandle, ResourceHandleParts, ResourceTypeId,
};
use crate::runtime_layer::HostFrames;
use crate::types::{ResourceType, ValueType};

use super::resource_record::ResourceRecord;
use super::store_id::StoreId;

/// Everything a store carries: the host's data of type `T`, and the
/// polyfill's own state beside it.
///
/// The core store the runtime layer gives the polyfill carries one
/// value of the embedder's choosing, and this is that value. The
/// host's own data is one field of it; the rest is the store as the
/// polyfill knows it — its identity, its handle tables, the
/// destructors its instances registered, and the scheduler with its
/// queues, its host tasks, and its suspend seam.
///
/// They sit here rather than beside the core store because of what a
/// host trampoline holds. The runtime layer hands a trampoline the
/// core store's context and nothing else, and it bounds every
/// trampoline `Send + Sync`, so nothing a trampoline captures may be
/// anything else — while an item runs against a store of a known
/// type and a host task's body need not be `Send` in the browser.
/// A trampoline reaches all of this through the context it is
/// handed, capturing none of it, which is what makes the scheduler
/// and its suspend seam reachable from inside a guest call. PDD018
/// asks for exactly that: a suspended guest thread resumes outside
/// any poll of a driver, so the scheduler's state has to be
/// reachable with no driver on the stack. Wasmtime's `StoreInner<T>`
/// is the same arrangement for the same reason.
///
/// The handle tables are the one part that also lives behind a
/// handle of its own. A resource trampoline and a lift/lower context
/// reach them without the store's context, and they hold nothing
/// that is not `Send`, so they can.
pub struct StoreData<T: 'static> {
    /// The host data the embedder gave the store.
    data: T,
    id: StoreId,
    tables: Arc<Mutex<HandleTables>>,
    destructors: HashMap<ResourceTypeId, ResourceDestructor<T>>,
    resource_types: HashMap<ResourceTypeId, LearnedName>,
    scheduler: Scheduler<T>,
    /// The provider that fills the store's suspend capability, when
    /// the engine selected one. It is installed as the store is
    /// constructed and stays for the store's whole life: nothing
    /// takes it out, a suspension included.
    provider: Option<StoreProvider>,
    /// Whether the store's owner dropped it while a thread the
    /// provider resumed had yet to run, which keeps the store
    /// allocated for that thread. The thread's shim reads the flag,
    /// has the store freed, and runs nothing else in it.
    dropped: bool,
    /// Whether a trap happened in the store. A poisoned store runs no
    /// more guest code: every host entry into a guest fails with the
    /// cannot-enter cause. Nothing clears the flag.
    poisoned: bool,
    /// The copy budget each crossing starts with, in bytes of host
    /// values: what Wasmtime calls the store's hostcall fuel.
    hostcall_fuel: usize,
    /// How many host functions of the polyfill run in the store now.
    host_frames: usize,
}

/// The copy budget a crossing starts with unless the host sets
/// another: 128 MiB, Wasmtime's default hostcall fuel.
const DEFAULT_HOSTCALL_FUEL: usize = 128 << 20;

/// A name the store learned for a resource type, with who taught it,
/// which is what settles whether a later name replaces it.
enum LearnedName {
    /// A component instantiated into this store imports or defines
    /// the resource type under this label. This is a name a user of
    /// the store can read in the component's own source.
    Component(ResourceType),
    /// No component in this store has named the resource type. This
    /// is the label a linker the store was instantiated from
    /// registered it under, kept so that an error about one of its
    /// handles renders something rather than nothing.
    Fallback(ResourceType),
}

impl LearnedName {
    /// The label itself, whoever taught it.
    fn label(&self) -> &ResourceType {
        match self {
            Self::Component(name) | Self::Fallback(name) => name,
        }
    }
}

impl<T: 'static> HostFrames for StoreData<T> {
    fn host_frames(&mut self) -> &mut usize {
        &mut self.host_frames
    }
}

impl<T: 'static> StoreData<T> {
    /// Construct the data of a fresh store around the host's `data`:
    /// a new identity, empty tables, no registered destructor, and
    /// nothing queued. Workspace-internal; not re-exported by
    /// `lib.rs`.
    pub fn new(data: T) -> Self {
        let scheduler = Scheduler::new();
        let mut tables = HandleTables::new();
        tables
            .tasks
            .count_host_tasks(scheduler.shared_host_call_count());
        Self {
            data,
            id: StoreId::fresh(),
            tables: Arc::new(Mutex::new(tables)),
            destructors: HashMap::new(),
            resource_types: HashMap::new(),
            scheduler,
            provider: None,
            dropped: false,
            poisoned: false,
            hostcall_fuel: DEFAULT_HOSTCALL_FUEL,
            host_frames: 0,
        }
    }

    /// The host data the embedder gave the store.
    pub fn host(&self) -> &T {
        &self.data
    }

    /// The host data the embedder gave the store, mutably.
    pub fn host_mut(&mut self) -> &mut T {
        &mut self.data
    }

    /// The copy budget each crossing of the store starts with, in
    /// bytes of host values. Workspace-internal.
    pub fn hostcall_fuel(&self) -> usize {
        self.hostcall_fuel
    }

    /// Set the copy budget each later crossing of the store starts
    /// with. Workspace-internal.
    pub fn set_hostcall_fuel(&mut self, fuel: usize) {
        self.hostcall_fuel = fuel;
    }

    /// The store's process-unique identity. Workspace-internal.
    pub fn id(&self) -> StoreId {
        self.id
    }

    /// The store's handle tables: one per component instance, shared
    /// by every handle kind, plus one per resource type for the
    /// host's own handles. Workspace-internal.
    pub fn tables(&self) -> &Arc<Mutex<HandleTables>> {
        &self.tables
    }

    /// Clone the handle for the per-store handle-tables ledger, for
    /// a trampoline or a lift/lower context to hold.
    /// Workspace-internal.
    pub fn tables_handle(&self) -> Arc<Mutex<HandleTables>> {
        self.tables.clone()
    }

    /// Lock the store's handle tables and record state.
    /// Workspace-internal.
    pub fn lock_tables(&self) -> Result<MutexGuard<'_, HandleTables>> {
        self.tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))
    }

    /// The store's cooperative scheduler: the ready queues, the host
    /// tasks, the entry gate, and the suspend seam.
    /// Workspace-internal.
    pub fn scheduler(&self) -> &Scheduler<T> {
        &self.scheduler
    }

    /// The store's cooperative scheduler, mutably.
    /// Workspace-internal.
    pub fn scheduler_mut(&mut self) -> &mut Scheduler<T> {
        &mut self.scheduler
    }

    /// Install the provider the engine selected. Workspace-internal.
    pub fn install_provider(&mut self, provider: StoreProvider) {
        self.provider = Some(provider);
    }

    /// The provider that fills the store's suspend capability, or
    /// `None` when the engine selected none. Workspace-internal.
    pub fn provider(&self) -> Option<&StoreProvider> {
        self.provider.as_ref()
    }

    /// Whether the owner dropped the store, which a resumed thread
    /// that runs on after it reads. Workspace-internal.
    pub fn dropped(&self) -> bool {
        self.dropped
    }

    /// Record that the owner dropped the store, and drop everything the
    /// polyfill keeps in it: the scheduler with its queued items, host
    /// tasks, and suspended threads, the provider, the destructors, and
    /// the handle tables with every record and end. No destructor runs.
    ///
    /// The runtime layer can keep the store allocated after its owner
    /// dropped it, for a guest call that runs on until it stops, as the
    /// browser's backend does for a call that runs on a microtask. The
    /// polyfill's state goes at once all the same, and such a call finds
    /// the mark, and nothing else of the store. The host's data drops
    /// with the store itself. Workspace-internal.
    pub fn mark_dropped(&mut self) {
        self.dropped = true;
        let scheduler = std::mem::take(&mut self.scheduler);
        let provider = self.provider.take();
        let destructors = core::mem::take(&mut self.destructors);
        let tables =
            core::mem::replace(&mut self.tables, Arc::new(Mutex::new(HandleTables::new())));
        drop((scheduler, provider, destructors, tables));
    }

    /// Whether a trap happened in the store. Workspace-internal.
    pub fn poisoned(&self) -> bool {
        self.poisoned
    }

    /// Record that a trap happened in the store. From here on no
    /// guest code of the store runs again. Workspace-internal.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }

    /// Record what the store knows about a resource type an
    /// instantiation introduced: the destructor to run when a handle
    /// to it is released, and the name an error about one of its
    /// handles renders.
    ///
    /// The name is optional because a resource type reaches the
    /// store from more than one place and not all of them carry one.
    /// A store that never learned a name says nothing about the type
    /// rather than inventing one for it. Workspace-internal.
    pub fn register_resource(
        &mut self,
        type_id: ResourceTypeId,
        name: Option<ResourceType>,
        destructor: ResourceDestructor<T>,
    ) {
        self.destructors.entry(type_id).or_insert(destructor);
        if let Some(name) = name {
            self.name_resource(type_id, name);
        }
    }

    /// Record the label a component instantiated into this store
    /// imports or defines `type_id` under, which is the name an
    /// error about one of its handles renders.
    ///
    /// One identity carries several names routinely. The shared
    /// resource-type identity of PDD013 is one case — a single host
    /// resource value registered against the label of two interfaces
    /// is one identity under two labels. A component that keeps a
    /// resource in several of its instances is another: it holds one
    /// table per instance, and each table names the resource as its
    /// own interface does. And a store takes more than one
    /// instantiation, so two components can name one identity
    /// differently.
    ///
    /// The store keeps one of those names rather than the set,
    /// because the `own<T>` an error renders names exactly one
    /// resource type: a set would have to collapse to a single name
    /// at every rendering, and collapsing it once, here, keeps the
    /// rule in one place.
    ///
    /// Which name that is follows a rule in two tiers. A label a
    /// component's own import or definition teaches — this method —
    /// outranks a label no component in the store has used, which
    /// [`StoreData::fallback_resource_name`] records; teaching one
    /// replaces a fallback already stored. Within a tier the first
    /// name taught wins, so the first component to bring an identity
    /// in is the one whose label the store keeps, however many
    /// components follow it. Workspace-internal.
    pub fn name_resource(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        if let Some(LearnedName::Component(_)) = self.resource_types.get(&type_id) {
            return;
        }
        self.resource_types
            .insert(type_id, LearnedName::Component(name));
    }

    /// Record a label to fall back on for `type_id` while no
    /// component in this store has named it: the label a linker
    /// registered a host resource under, which is all an error has
    /// to render when nothing has imported the resource.
    ///
    /// It is the lower of the two tiers [`StoreData::name_resource`]
    /// describes. It never displaces a name a component taught, and
    /// a component that names the identity later displaces it.
    /// Workspace-internal.
    pub fn fallback_resource_name(&mut self, type_id: ResourceTypeId, name: ResourceType) {
        self.resource_types
            .entry(type_id)
            .or_insert(LearnedName::Fallback(name));
    }

    /// What the store knows about `type_id` at this moment: whether
    /// a destructor is registered for it, and the name it renders
    /// for it with the tier that taught it.
    ///
    /// An instantiation takes one record per resource type before it
    /// registers anything, and hands the records back through
    /// [`StoreData::restore_resource`] when its plan fails, so a
    /// failed instantiation leaves the store as it found it.
    /// Workspace-internal.
    pub fn resource_record(&self, type_id: ResourceTypeId) -> ResourceRecord {
        let (taught_name, fallback_name) = match self.resource_types.get(&type_id) {
            Some(LearnedName::Component(name)) => (Some(name.clone()), None),
            Some(LearnedName::Fallback(name)) => (None, Some(name.clone())),
            None => (None, None),
        };
        ResourceRecord {
            type_id,
            destructor: self.destructors.contains_key(&type_id),
            taught_name,
            fallback_name,
        }
    }

    /// Put back what the store knew about one resource type before
    /// an instantiation registered it.
    ///
    /// A destructor the record did not hold is forgotten, because
    /// the registration that added it is the one being taken back; a
    /// destructor the store already had is left alone, because
    /// registering one never displaces it. The name goes back to the
    /// label the record holds, in the tier the record holds it in,
    /// and a store that had learned no name for the type learns none
    /// from the attempt. Workspace-internal.
    pub fn restore_resource(&mut self, record: ResourceRecord) {
        if !record.destructor {
            self.destructors.remove(&record.type_id);
        }
        self.resource_types.remove(&record.type_id);
        if let Some(name) = record.taught_name {
            self.name_resource(record.type_id, name);
        } else if let Some(name) = record.fallback_name {
            self.fallback_resource_name(record.type_id, name);
        }
    }

    /// How many resource types the store has a destructor
    /// registered for. Workspace-internal.
    pub fn registered_destructors(&self) -> usize {
        self.destructors.len()
    }

    /// How many resource types the store has learned a name for,
    /// whether a component taught it or a linker's sweep left it as
    /// a fallback. Workspace-internal.
    pub fn learned_resource_names(&self) -> usize {
        self.resource_types.len()
    }

    /// The name the store renders for the resource type `type_id`,
    /// when it learned one. Workspace-internal.
    pub fn resource_type(&self, type_id: ResourceTypeId) -> Option<ResourceType> {
        self.resource_types
            .get(&type_id)
            .map(|learned| learned.label().clone())
    }

    /// The destructor recorded for `type_id`, if an instance
    /// registered one. Workspace-internal.
    pub fn destructor(&self, type_id: ResourceTypeId) -> Option<ResourceDestructor<T>> {
        self.destructors.get(&type_id).cloned()
    }

    /// Whether a component defines the resource type, as far as the
    /// store knows: an instance registered an in-binary destructor
    /// for it. Read for the wrong-type trap message.
    /// Workspace-internal.
    pub fn is_guest_defined(&self, type_id: ResourceTypeId) -> bool {
        matches!(
            self.destructors.get(&type_id),
            Some(ResourceDestructor::Local { .. })
        )
    }

    /// Mint a fresh `own<T>` handle in this store's host table for
    /// `type_id`, with `rep` as its representation.
    /// Workspace-internal.
    pub fn resource_new(&self, type_id: ResourceTypeId, rep: u32) -> Result<ResourceHandle> {
        let guest_defined = self.is_guest_defined(type_id);
        let mut guard = self.lock_tables()?;
        let table = guard.host_table(type_id);
        let index = guard.insert_own(table, type_id, guest_defined, rep);
        Ok(ResourceHandleParts {
            type_id,
            index,
            rep,
        }
        .into())
    }

    /// Take the entry of a handle the host holds out of its table,
    /// and report the rep the destructor runs against.
    /// Workspace-internal.
    pub fn remove_host_handle(&self, handle: ResourceHandle) -> Result<u32> {
        let guest_defined = self.is_guest_defined(handle.type_id());
        let mut guard = self.lock_tables()?;
        let table = guard.host_table(handle.type_id());
        guard
            .remove_own(table, handle.index(), handle.type_id(), guest_defined)
            .map_err(|e| {
                let reason = match e {
                    HandleLookupError::Unknown { index } => {
                        format!("handle index {index} is not live in the host's resource table")
                    }
                    HandleLookupError::NotOwned { index } => {
                        format!("handle index {index} is a borrow, which returns with its call")
                    }
                    other => other.to_string(),
                };
                Error::from(AbiError {
                    position: AbiPosition::Argument(0),
                    valtype: self.resource_type(handle.type_id()).map(ValueType::Own),
                    cause: AbiCause::InvalidHandle { reason },
                })
            })
    }

    /// Whether a turn of this store is running. A driver entered
    /// while another driver of the same store is inside a turn fails
    /// with the recursive-driver cause.
    ///
    /// Both driver entries ask this before either of them has a
    /// [`TurnGuard`], so it is asked through the guard, which reads
    /// past a poison a panic left rather than refusing the question.
    /// A panic that poisoned the tables while no turn was running
    /// would otherwise refuse every later driver of the store. The
    /// question is only a question: the poison is cleared by the
    /// turn the driver goes on to enter, not here.
    /// Workspace-internal.
    pub fn turn_in_flight(&self) -> bool {
        TurnGuard::in_turn(&self.tables)
    }

    /// Whether the store holds work only a turn can carry forward:
    /// an item in one of the ready queues, deferred work included,
    /// or a host task that has not resolved. A driver that parked on
    /// such a store would wait for a wake that the work it parked on
    /// is what produces. Workspace-internal.
    pub fn has_pending_work(&self) -> bool {
        self.has_ready_item() || self.scheduler.host_task_count() > 0
    }

    /// Whether the store holds an item a turn would run: one in one
    /// of the ready queues, deferred work included. This is the half
    /// of [`has_pending_work`](Self::has_pending_work) that a driver
    /// already told a host task is pending wants, because the whole
    /// of it would always answer yes there. Workspace-internal.
    pub fn has_ready_item(&self) -> bool {
        self.scheduler.has_ready_item()
    }

    /// The waker of the turn that is running, or a waker that does
    /// nothing when no turn is running — which is the case for a
    /// thread resumed outside any poll of a driver.
    ///
    /// Two callers want it. A trampoline that starts a host task
    /// polls its body once before it returns to the guest, and a
    /// body polled outside a turn is not lost all the same, because
    /// a host task that joins the store counts as woken and the next
    /// turn polls it. The suspend seam's nested turn wants it for a
    /// like reason: it polls with the waker the outer turn recorded
    /// rather than recording one of its own, so the wake of a host
    /// task it leaves pending reaches the waker the executor already
    /// holds.
    /// Workspace-internal.
    pub fn active_waker(&self) -> Waker {
        self.tables
            .lock()
            .ok()
            .and_then(|guard| guard.scheduler.active_waker())
            .unwrap_or_else(|| Waker::noop().clone())
    }

    /// Why a driver whose store went idle with work left failed: the
    /// cannot-block cause when any instance of the store must not
    /// suspend, and the deadlock cause when none must.
    ///
    /// This is the one rule that reads every instance of the store
    /// rather than the blocked thread's own, and it is how Wasmtime
    /// names the error of an idle store (`any_may_not_suspend` in
    /// `concurrent.rs`): an instance that must not suspend has a
    /// synchronous call in progress that the idle store can no longer
    /// return from, and that call failing to return is what went
    /// wrong. The call need not be the driver's own. A thread that
    /// must not block can suspend as a switcher, so an idle store can
    /// hold another instance's synchronous call as well.
    ///
    /// A thread suspended in the provider that the idle store can no
    /// longer resume fails with the same cause, which reaches the call
    /// it belongs to. Workspace-internal.
    pub fn idle_cause(&self) -> SchedulerCause {
        let any_may_not_suspend = self
            .tables
            .lock()
            .map(|guard| {
                guard
                    .tasks
                    .instances()
                    .iter()
                    .any(|record| record.may_not_suspend)
            })
            .unwrap_or(false);
        if any_may_not_suspend {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::Deadlock
        }
    }

    /// Why a suspension whose nested turns gave up with the
    /// condition unmet failed. Five rules, in this order.
    ///
    /// The blocked task's own instance, with a synchronous call in
    /// progress, gives the cannot-block cause. The flag is the
    /// may-not-suspend flag of the instance record. The task was
    /// given the ready work of its instance and found the condition
    /// still unmet, which is the reference's rule for it. A start
    /// intrinsic clears the flag on an `async`-typed callee's
    /// instance while it runs the callee, as Wasmtime does, so a
    /// callee that reenters an instance with a synchronous call in
    /// progress is not held to that call's rule. A nested turn held
    /// to one instance reads this rule through
    /// [`suspend_cause_in`](Self::suspend_cause_in) instead.
    ///
    /// The other four rules are
    /// [`cause_past_cannot_block`](Self::cause_past_cannot_block).
    /// Workspace-internal.
    pub fn suspend_cause(&self) -> SchedulerCause {
        if self.must_not_block_instance().is_some() {
            SchedulerCause::CannotBlock
        } else {
            self.cause_past_cannot_block()
        }
    }

    /// Why a nested turn held to `instance` went idle with its
    /// condition unmet: the blocked thread's own instance must not
    /// suspend, and the turn ran every ready thread of that instance
    /// it could reach.
    ///
    /// The cannot-block cause is read from that instance alone. It
    /// holds when no other thread of the instance is ready, which is
    /// where the reference's `canon_lift` traps a sync-typed call
    /// that blocked. A thread of the instance that is ready and still
    /// did not run waits on the real stack below the blocked thread,
    /// where only a stack switch would reach it, and the other causes
    /// then decide, in the order [`suspend_cause`](Self::suspend_cause)
    /// states them. Workspace-internal.
    pub fn suspend_cause_in(&self, instance: InstanceId) -> SchedulerCause {
        let ready = self
            .tables
            .lock()
            .map(|guard| guard.tasks.other_thread_ready_in(instance))
            .unwrap_or(false);
        if ready {
            self.cause_past_cannot_block()
        } else {
            SchedulerCause::CannotBlock
        }
    }

    /// The cause of a failed block once the blocked thread's own
    /// instance does not give the cannot-block cause. The other four
    /// rules: two that frames below the block decide, then two that
    /// the store decides.
    ///
    /// A frame below the blocked thread decides first, read in one
    /// walk from the innermost frame out
    /// ([`TaskTables::cause_below`](crate::concurrency::TaskTables::cause_below)).
    /// A frame that would go on under a stack switch gives the
    /// stack-switch cause: a start intrinsic ran a callee from inside
    /// its own frame and the caller that started it would go on once
    /// control returned to it, or a thread built-in switched to a
    /// thread that is now running above a thread that is not
    /// suspended. The store cannot see that work, because it is guest
    /// code on the real stack rather than an item or a host task, so
    /// an idle store here does not mean nothing can move. Only the
    /// target's capability is missing. Wasmtime runs such a callee on
    /// a fiber of its own, and the same shapes do not fail there. A
    /// caller below that waits for the blocked callee in an instance
    /// that must not suspend, with no other thread of that instance
    /// ready, gives the cannot-block cause: its wait is a block of its
    /// own instance, which the reference's `canon_lift` traps. Of the
    /// two, the frame nearer the block counts.
    ///
    /// A host task that has not resolved gives the stack-switch
    /// cause, because the reference permits that block and only the
    /// target has no provider to serve it. The rule reads the
    /// store's host tasks and nothing else. The future of a
    /// synchronous lower counts through them: the lower parks its
    /// future there for as long as the call waits on it.
    ///
    /// Otherwise the deadlock cause: no frame below the block would go
    /// on, and the store holds nothing that can meet the condition. An
    /// item a turn is still holding back does not change that answer:
    /// a nested turn runs every item it is allowed to run, so an item
    /// left over is one no turn of this store can release. A
    /// synchronous call into an instance that no caller below waits in
    /// does not change it either.
    fn cause_past_cannot_block(&self) -> SchedulerCause {
        let below = self
            .tables
            .lock()
            .ok()
            .and_then(|guard| guard.tasks.cause_below());
        match below {
            Some(cause) => cause,
            None if self.host_future_pending() => SchedulerCause::StackSwitchNeeded,
            None => SchedulerCause::Deadlock,
        }
    }

    /// Whether a host future that can still resolve is pending: the
    /// store holds a host task, the parked future of a synchronous
    /// lower included.
    fn host_future_pending(&self) -> bool {
        self.scheduler.host_future_pending()
    }

    /// The instance whose ready work is the whole of what a nested
    /// turn run for the current task may run, or `None` when that
    /// task is allowed to block and the turn runs every ready item.
    ///
    /// A task that must not block gives way only to the ready work
    /// of its own instance, which is what Wasmtime switches to
    /// before it raises the cannot-block trap. A task that belongs
    /// to no instance is allowed to block, as `must_not_block` says.
    /// Workspace-internal.
    pub fn must_not_block_instance(&self) -> Option<InstanceId> {
        let guard = self.tables.lock().ok()?;
        let task = guard.tasks.current_task()?;
        let instance = guard.tasks.task(task)?.instance?;
        guard
            .tasks
            .instance(instance)?
            .may_not_suspend
            .then_some(instance)
    }
}
