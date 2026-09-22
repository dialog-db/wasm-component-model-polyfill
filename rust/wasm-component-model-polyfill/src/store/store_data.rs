//! Everything a store carries, the host's data and the polyfill's
//! own.

use core::task::Waker;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::concurrency::{InstanceId, Scheduler, TaskId, TurnGuard};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::internal::ErrorInternal;
use crate::resource::{
    HandleLookupError, HandleTables, ResourceHandle, ResourceHandleParts, ResourceTypeId,
};
use crate::types::{ResourceType, ValueType};

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
}

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

impl<T: 'static> StoreData<T> {
    /// Construct the data of a fresh store around the host's `data`:
    /// a new identity, empty tables, no registered destructor, and
    /// nothing queued. Workspace-internal; not re-exported by
    /// `lib.rs`.
    pub fn new(data: T) -> Self {
        Self {
            data,
            id: StoreId::fresh(),
            tables: Arc::new(Mutex::new(HandleTables::new())),
            destructors: HashMap::new(),
            resource_types: HashMap::new(),
            scheduler: Scheduler::new(),
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
    /// body polled outside a turn counts as woken all the same,
    /// because the next turn polls every host task the store holds.
    /// The suspend seam's nested turn wants it for the same reason:
    /// it polls with the waker the outer turn recorded rather than
    /// recording one of its own, so a host task it leaves pending
    /// carries the waker the executor already holds.
    /// Workspace-internal.
    pub fn active_waker(&self) -> Waker {
        self.tables
            .lock()
            .ok()
            .and_then(|guard| guard.scheduler.active_waker())
            .unwrap_or_else(|| Waker::noop().clone())
    }

    /// Why a driver that went idle failed: the cannot-block cause
    /// when the task it waits on is one that must not block, and the
    /// deadlock cause otherwise.
    ///
    /// While no suspend provider is installed — the seam's slot is
    /// empty on both targets today — the task the driver waits on is
    /// the only call that can be in flight here. A synchronous call
    /// between two components holds a native frame for its whole
    /// length, and a driver is polled with no such frame under it,
    /// so no other instance can be inside a call that must return.
    /// The nested turn of the suspend seam is the path that runs
    /// under one, and [`suspend_cause`](Self::suspend_cause) is what
    /// answers there. A provider that switched stacks would lift
    /// that frame off the driver's, and the driver would then have
    /// to serve the same rules `suspend_cause` does.
    /// Workspace-internal.
    pub fn idle_cause(&self, task: Option<TaskId>) -> SchedulerCause {
        if self.must_not_block(task) {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::Deadlock
        }
    }

    /// Why a suspension whose nested turns gave up with the
    /// condition unmet failed. Three rules, in this order.
    ///
    /// Any instance of the store with a synchronous call in
    /// progress gives the cannot-block cause. The flag is the
    /// may-not-suspend flag of the instance record, read across
    /// every instance rather than on the blocked task's own: a
    /// synchronous caller reaches an `async`-typed callee through a
    /// synchronous call, and the callee is allowed to block while
    /// its caller is not, so the callee blocking for ever is the
    /// caller failing to return and the cause names the caller's
    /// rule. It is also the reference's own rule for the blocked
    /// task itself, which was given the ready work of its instance
    /// and found the condition still unmet. This is what Wasmtime
    /// reports where it would otherwise raise its deadlock trap.
    ///
    /// A host task that has not resolved gives the stack-switch
    /// cause, because the reference permits that block and only the
    /// target has no provider to serve it. The future of a call
    /// that blocked on one of its own counts: it is pending, it can
    /// still resolve, and it is in the frame that blocked rather
    /// than in the store.
    ///
    /// An idle store gives the deadlock cause, because nothing left
    /// in it can ever meet the condition. An item a turn is still
    /// holding back does not change that answer: a nested turn runs
    /// every item it is allowed to run, so an item left over is one
    /// no turn of this store can release. Workspace-internal.
    pub fn suspend_cause(&self) -> SchedulerCause {
        if self.any_must_not_block() {
            SchedulerCause::CannotBlock
        } else if self.host_future_pending() {
            SchedulerCause::StackSwitchNeeded
        } else {
            SchedulerCause::Deadlock
        }
    }

    /// Whether a host future that can still resolve is pending: one
    /// of the store's host tasks, or the future of a call that
    /// blocked on it, which stays in the frame that started it.
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

    /// Whether `task` runs in an instance that forbids its threads
    /// to suspend. A store whose tables are unreachable answers
    /// `false`, so a poisoned lock never turns a deadlock into a
    /// cannot-block.
    fn must_not_block(&self, task: Option<TaskId>) -> bool {
        let Ok(guard) = self.tables.lock() else {
            return false;
        };
        task.and_then(|task| guard.tasks.task(task))
            .and_then(|record| record.instance)
            .and_then(|instance| guard.tasks.instance(instance))
            .map(|record| record.may_not_suspend)
            .unwrap_or(false)
    }

    /// Whether any instance of the store forbids its threads to
    /// suspend, which says that some call in flight must return
    /// before the store may block. A store whose tables are
    /// unreachable answers `false`, as `must_not_block` does.
    fn any_must_not_block(&self) -> bool {
        let Ok(guard) = self.tables.lock() else {
            return false;
        };
        guard
            .tasks
            .instances()
            .iter()
            .any(|record| record.may_not_suspend)
    }
}
