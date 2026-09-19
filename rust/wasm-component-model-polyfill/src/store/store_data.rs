//! Everything a store carries, the host's data and the polyfill's
//! own.

use core::task::Waker;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::concurrency::{InstanceId, Scheduler, TaskId, TurnGuard};
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result, SchedulerCause};
use crate::executor::ResourceDestructor;
use crate::resource::{HandleLookupError, HandleTables, ResourceHandle, ResourceTypeId};
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
    scheduler: Scheduler<T>,
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

    /// Record the destructor of a resource type an instance
    /// introduced. Workspace-internal.
    pub fn register_destructor(
        &mut self,
        type_id: ResourceTypeId,
        destructor: ResourceDestructor<T>,
    ) {
        self.destructors.entry(type_id).or_insert(destructor);
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
        Ok(ResourceHandle {
            type_id,
            index,
            rep,
        })
    }

    /// Take the entry of a handle the host holds out of its table,
    /// and report the rep the destructor runs against.
    /// Workspace-internal.
    pub fn remove_host_handle(&self, handle: ResourceHandle) -> Result<u32> {
        let guest_defined = self.is_guest_defined(handle.type_id);
        let mut guard = self.lock_tables()?;
        let table = guard.host_table(handle.type_id);
        guard
            .remove_own(table, handle.index, handle.type_id, guest_defined)
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
                    valtype: Some(ValueType::Own(ResourceType::new("resource"))),
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

    /// Why a nested turn that gave up with its condition unmet
    /// failed.
    ///
    /// A task that must not block gets the cannot-block cause, which
    /// is the rule of the reference: it was given the ready work of
    /// its own instance and that work did not meet the condition.
    ///
    /// A task that is allowed to block gets the stack-switch cause
    /// while the store still holds work — a host task that has not
    /// resolved, or an item only a driver's turn may run — because
    /// the reference permits that block and only the target has no
    /// provider to serve it.
    ///
    /// A task that is allowed to block and finds the store idle gets
    /// the cannot-block cause when any instance is inside a
    /// synchronous call that has not returned, and the deadlock
    /// cause otherwise. This is the rule Wasmtime applies where it
    /// would raise its deadlock trap. A synchronous caller reaches
    /// an `async`-typed callee through a synchronous call, and the
    /// callee is allowed to block while its caller is not: the
    /// callee blocking forever is the caller failing to return, so
    /// the cause names the caller's rule. Workspace-internal.
    pub fn suspend_cause(&self) -> SchedulerCause {
        let current = self
            .tables
            .lock()
            .ok()
            .and_then(|guard| guard.tasks.current_task());
        if self.must_not_block(current) {
            SchedulerCause::CannotBlock
        } else if self.has_pending_work() {
            SchedulerCause::StackSwitchNeeded
        } else if self.any_must_not_block() {
            SchedulerCause::CannotBlock
        } else {
            SchedulerCause::Deadlock
        }
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
