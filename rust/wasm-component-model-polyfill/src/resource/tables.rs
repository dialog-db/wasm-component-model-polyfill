//! Per-store collection of handle tables, keyed by table identity,
//! with the store's task, subtask, and thread records beside them.
//!
//! Each [`Store`] owns one `HandleTables` instance. A component
//! instance's table is shared by every handle kind the instance
//! uses; the collection lazily creates a [`HandleTable`] the first
//! time a given [`TableId`] is touched, so a table nothing has
//! allocated into yet costs nothing. The host gets its own table
//! per resource type, also created on first use.
//!
//! The collection also carries [`TaskTables`], the store's records
//! of the calls in flight and of the waitables a guest waits on. The
//! two live together because the borrow and waitable operations need
//! both: a borrow lowered into a guest takes an index in a handle
//! table and counts against the current task, a borrow lifted out of
//! an owning entry raises that entry's lend count and is recorded on
//! the record of the call it was lent for, and delivering that
//! call's result lowers those counts again.
//!
//! [`HandleTables::lend_to`] states where a lend lives and when it
//! comes back, for every direction a call crosses in.
//!
//! Workspace-internal: the collection is reached only through
//! crate-private accessors on [`Store`]. The public API exposes the
//! handles themselves through the [`Val`] family rather than the
//! tables.
//!
//! [`Store`]: crate::Store
//! [`Val`]: crate::Val

use std::collections::HashMap;

use crate::concurrency::{
    EndId, EndKind, Event, SchedulerState, Scope, SubtaskId, SubtaskState, TaskId, TaskTables,
    ThreadId, WaitableId, WaitableSetId,
};
use crate::error::Error;
use crate::internal::ErrorInternal;
use crate::value::Val;

use super::TaskEnd;
use super::handle_kind::HandleKind;
use super::handle_lookup_error::HandleLookupError;
use super::identity::ResourceTypeId;
use super::table::HandleTable;
use super::table_id::TableId;

/// Every handle table a [`Store`] carries, with the task, subtask,
/// and thread records the canonical ABI keeps for borrows.
///
/// [`Store`]: crate::Store
pub struct HandleTables {
    /// Every table in the store, by identity: one per component
    /// instance, shared by every handle kind, plus one per resource
    /// type for the host's own handles.
    tables: HashMap<TableId, HandleTable>,
    /// The host's table for each resource type, created on first use.
    host_tables: HashMap<ResourceTypeId, TableId>,
    /// The store's task, subtask, thread, and instance records, with
    /// the stack of current scopes. Workspace-internal.
    pub tasks: TaskTables,
    /// The half of the store's scheduler a trampoline can reach: the
    /// waker of the turn that is running, and whether a turn is
    /// running at all. It lives here because a trampoline reaches
    /// the collection from inside a runtime-layer closure, where the
    /// store itself is unreachable. Workspace-internal.
    pub scheduler: SchedulerState,
}

impl HandleTables {
    /// Construct an empty collection.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            host_tables: HashMap::new(),
            tasks: TaskTables::new(),
            scheduler: SchedulerState::new(),
        }
    }

    /// End the task `task` on its success path: every borrow lowered
    /// into the guest during the task must have been dropped, else
    /// the residual count is returned and the scope is still popped.
    /// Each lend recorded against the task is undone, the
    /// may-not-suspend flag the enter intrinsic saved is restored,
    /// and the task's record is removed.
    ///
    /// A scope a failed call left above `task` is discarded first, so
    /// a failure between a push and its pop cannot strand a scope or
    /// the lends recorded against it. Does nothing when `task` is not
    /// on the stack, and [`TaskEnd::Untouched`] is what says so: a
    /// caller with a second half to run — the scheduler's sweep of
    /// what the task still has queued — must run it only for a task
    /// that ended.
    ///
    /// The steps run through [`TaskExit`], which finishes whichever
    /// of them a panic interrupted, for the reason that type's
    /// documentation gives.
    pub fn exit_task(&mut self, task: TaskId) -> TaskEnd {
        if !self.tasks.scopes().contains(&Scope::Task(task)) {
            return TaskEnd::Untouched;
        }
        TaskEnd::Ended(TaskExit::begin(self, task).finish())
    }

    /// End `task` whether or not its scope is still on the stack, on
    /// its success path. Behaves as [`exit_task`](Self::exit_task) in
    /// every other respect.
    ///
    /// The callback loop of an asynchronous export ends a task this
    /// way. Such a task is entered and left once per event: the scope
    /// is pushed when core code runs and popped when it returns, and
    /// the status word it returned is what says whether the task is
    /// over. The exit therefore comes after the pop, with no scope
    /// left to unwind. There is no stack to consult, so the end is
    /// always [`TaskEnd::Ended`].
    pub fn end_task(&mut self, task: TaskId) -> TaskEnd {
        TaskEnd::Ended(TaskExit::begin(self, task).finish())
    }

    /// Resolve `task` with `result`: every handle lent for the call
    /// the task is serving comes back, and the record takes the
    /// result for whoever is waiting on it. Answers whether the
    /// store still held a record to resolve.
    ///
    /// The lends come back here rather than at the task's exit
    /// because the resolution is when the caller takes delivery of
    /// the result, which is the rule
    /// [`lend_to`](Self::lend_to) states. A callback export can
    /// `task.return` and keep running, and the host that called it
    /// has its result the moment it does, so a handle the host lent
    /// for the call cannot stay lent for the rest of the task. A
    /// task that resolves and later exits finds its lender list
    /// already empty, so the exit gives nothing back twice.
    pub fn resolve_task(&mut self, task: TaskId, result: Option<Val>) -> bool {
        self.undo_lends(Scope::Task(task));
        match self.tasks.task_mut(task) {
            Some(record) => {
                record.resolve(result);
                true
            }
            None => false,
        }
    }

    /// Pop `task`'s scope without ending the task: the record stays
    /// in the store and the task can be entered again.
    ///
    /// This is what the callback loop of an asynchronous export does
    /// when core code returns. Every scope the core code left above
    /// the task is discarded with it, under the rule
    /// [`exit_task`](Self::exit_task) states.
    pub fn leave_task_scope(&mut self, task: TaskId) {
        while !self.unwind_one(Scope::Task(task)) {}
    }

    /// End the innermost task on the stack on its success path, for
    /// the one caller that cannot name the task it pushed: an
    /// adapter's enter and exit intrinsics are two separate calls
    /// that pass no identity between them. Behaves as
    /// [`exit_task`](Self::exit_task) in every other respect. An
    /// empty stack has no task to end, which is
    /// [`TaskEnd::Untouched`] too.
    pub fn exit_current_task(&mut self) -> TaskEnd {
        match self.tasks.current_task() {
            Some(task) => self.exit_task(task),
            None => TaskEnd::Untouched,
        }
    }

    /// End the task `task` on its failure path: the scope is popped
    /// and its lends undone, and no borrow check is made, because
    /// the call already failed. Every scope the failure left above
    /// `task` is discarded with it.
    ///
    /// Whether the task ended comes back, under the rule
    /// [`exit_task`](Self::exit_task) states: a failure that names a
    /// task whose scope is not on the stack ends nothing, and its
    /// caller must leave what that task has queued where it is. The
    /// count the exit read is dropped rather than reported, because
    /// there is no borrow check on this path.
    pub fn abandon_task(&mut self, task: TaskId) -> bool {
        self.exit_task(task).ended()
    }

    /// Deliver the subtask `subtask`'s resolution and pop it: the
    /// subtask moves to `state`, the count on each handle it
    /// borrowed is decremented, and its record is removed. A scope a
    /// failed call left above it is discarded first, and nothing
    /// happens when `subtask` is not on the stack.
    ///
    /// The steps run through [`SubtaskExit`], which finishes
    /// whichever of them a panic interrupted, for the reason that
    /// type's documentation gives.
    pub fn exit_subtask(&mut self, subtask: SubtaskId, state: SubtaskState) {
        if !self.tasks.scopes().contains(&Scope::Subtask(subtask)) {
            return;
        }
        SubtaskExit::begin(self, subtask, state).finish();
    }

    /// End the subtask `subtask` on its failure path: the call never
    /// returned, so its resolution is a cancellation — before the
    /// callee read its parameters when the failure was in lifting
    /// them, and after when the host function itself failed. The
    /// handles the caller lent are given back either way.
    pub fn abandon_subtask(&mut self, subtask: SubtaskId) {
        let state = match self.tasks.subtask(subtask).map(|record| record.state) {
            Some(SubtaskState::Starting) => SubtaskState::CancelledBeforeStarted,
            _ => SubtaskState::CancelledBeforeReturned,
        };
        self.exit_subtask(subtask, state);
    }

    /// Deliver the resolution of `subtask`: the count on each handle
    /// the call borrowed is decremented and the subtask is marked as
    /// having delivered its resolution, which is what lets a guest
    /// drop it. A subtask that has not resolved has no resolution to
    /// deliver, and a second delivery does nothing.
    pub fn deliver_subtask_resolution(&mut self, subtask: SubtaskId) -> Result<(), Error> {
        let record = self
            .tasks
            .subtask(subtask)
            .ok_or_else(|| Error::internal("subtask record is not in the store"))?;
        if !record.state.resolved() {
            return Err(Error::internal(
                "a subtask's resolution was delivered before the call resolved",
            ));
        }
        self.deliver_resolution(subtask);
        Ok(())
    }

    /// Give back the handles `subtask` borrowed, once. Does nothing
    /// for a subtask whose resolution was already delivered, or one
    /// whose record is gone.
    fn deliver_resolution(&mut self, subtask: SubtaskId) {
        let delivered = self
            .tasks
            .subtask(subtask)
            .map(|record| record.resolve_delivered)
            .unwrap_or(true);
        if delivered {
            return;
        }
        self.undo_lends(Scope::Subtask(subtask));
        if let Some(record) = self.tasks.subtask_mut(subtask) {
            record.resolve_delivered = true;
        }
    }

    /// The waitable the entry at `index` of `table` names. A built-in
    /// that takes a waitable handle reaches its record this way.
    pub fn waitable_from_handle(
        &self,
        table: TableId,
        index: u32,
    ) -> Result<WaitableId, HandleLookupError> {
        match self.entry(table, index) {
            Some(HandleKind::Subtask { subtask }) => Ok(self.tasks.subtask_waitable(subtask)),
            Some(entry) => match entry.as_end() {
                Some((kind, end)) => Ok(WaitableId::from_end(kind, end)),
                None => Err(HandleLookupError::NotAWaitable { index }),
            },
            None => Err(HandleLookupError::Unknown { index }),
        }
    }

    /// The end the entry at `index` of `table` names, when the entry
    /// is an end of kind `kind`. A built-in that takes a stream or
    /// future end reaches its record this way, and each names the one
    /// kind it works on.
    pub fn end_from_handle(
        &self,
        table: TableId,
        index: u32,
        kind: EndKind,
    ) -> Result<EndId, HandleLookupError> {
        match self.entry(table, index) {
            Some(entry) => match entry.as_end() {
                Some((found, end)) if found == kind => Ok(end),
                _ => Err(HandleLookupError::NotAnEnd {
                    index,
                    expected: kind,
                }),
            },
            None => Err(HandleLookupError::Unknown { index }),
        }
    }

    /// The subtask the entry at `index` of `table` names.
    /// `subtask.drop` reaches its record this way, and it is the one
    /// built-in that takes a subtask handle rather than a waitable
    /// one.
    pub fn subtask_from_handle(
        &self,
        table: TableId,
        index: u32,
    ) -> Result<SubtaskId, HandleLookupError> {
        match self.entry(table, index) {
            Some(HandleKind::Subtask { subtask }) => Ok(subtask),
            Some(_) => Err(HandleLookupError::NotASubtask { index }),
            None => Err(HandleLookupError::Unknown { index }),
        }
    }

    /// The waitable set the entry at `index` of `table` names. A
    /// built-in that takes a waitable-set handle reaches its record
    /// this way.
    pub fn waitable_set_from_handle(
        &self,
        table: TableId,
        index: u32,
    ) -> Result<WaitableSetId, HandleLookupError> {
        match self.entry(table, index) {
            Some(HandleKind::WaitableSet { set }) => Ok(set),
            Some(_) => Err(HandleLookupError::NotAWaitableSet { index }),
            None => Err(HandleLookupError::Unknown { index }),
        }
    }

    /// Take the event pending on `waitable`, leaving its slot empty.
    /// Taking a subtask's event also delivers the subtask's
    /// resolution when the call has resolved, which is what gives the
    /// handles it borrowed back to the caller.
    ///
    /// The two halves are one operation, so the resolution is
    /// delivered before the slot is emptied: the record operation
    /// that empties it refuses a subtask whose resolution is still
    /// owed, and this is the only path that pays it first.
    pub fn take_event(&mut self, waitable: WaitableId) -> Result<Option<Event>, Error> {
        if !self.tasks.has_pending_event(waitable)? {
            return Ok(None);
        }
        if let WaitableId::Subtask(subtask) = waitable {
            let resolved = self
                .tasks
                .subtask(subtask)
                .map(|record| record.state.resolved())
                .unwrap_or(false);
            if resolved {
                self.deliver_resolution(subtask);
            }
        }
        self.tasks.take_pending_event(waitable)
    }

    /// Poll `set`: deliver the event of the waitable that joined
    /// earliest among those that hold one, and answer the none event
    /// when the set holds none. A poll never blocks.
    pub fn poll_waitable_set(&mut self, set: WaitableSetId) -> Result<Event, Error> {
        match self.tasks.next_ready_waitable(set)? {
            Some(waitable) => Ok(self.take_event(waitable)?.unwrap_or_else(Event::none)),
            None => Ok(Event::none()),
        }
    }

    /// Wait on `set` with `thread`. A set that already holds an event
    /// delivers it at once and the thread does not block. Otherwise
    /// the thread is parked on the set and `None` says the caller
    /// must suspend it; the thread's wait ends at
    /// [`finish_wait_on_waitable_set`](Self::finish_wait_on_waitable_set).
    ///
    /// Both a blocking `waitable-set.wait` and a callback that
    /// returned the wait code with `set` come here: the record-level
    /// effect is the same, and only how the thread gives way differs.
    pub fn wait_on_waitable_set(
        &mut self,
        set: WaitableSetId,
        thread: ThreadId,
    ) -> Result<Option<Event>, Error> {
        if self.tasks.set_has_pending_event(set)? {
            return Ok(Some(self.poll_waitable_set(set)?));
        }
        self.tasks.begin_wait(set, thread)?;
        Ok(None)
    }

    /// End the wait `wait_on_waitable_set` parked `thread` for and
    /// deliver what the set holds. Answers the none event when it
    /// holds nothing, which is what a thread resumed for another
    /// reason sees.
    pub fn finish_wait_on_waitable_set(
        &mut self,
        set: WaitableSetId,
        thread: ThreadId,
    ) -> Result<Event, Error> {
        self.tasks.end_wait(set, thread)?;
        self.poll_waitable_set(set)
    }

    /// Pop one scope on the way down to `scope`, discarding it when
    /// it is not `scope` itself, and answer whether the unwind has
    /// finished. It has finished when `scope` is no longer on the
    /// stack, either because an earlier call took it or because it
    /// never was there.
    ///
    /// The scopes above `scope` are what a call that failed between
    /// its own push and its own pop left behind: the failure travels
    /// as an error or a trap past the pop that would have ended the
    /// scope, so the scope that catches it ends them.
    ///
    /// The unwind is one pop per call so that an exit guard can own
    /// it. The pop takes the scope off the stack before the discard
    /// that can panic runs, so a panic in a discard leaves the
    /// scopes below it still to pop, and the guard's drop resumes
    /// the unwind there. An unwind that popped them all at once
    /// could not be resumed that way: a guard that marked such a
    /// step taken would strand the exiting scope's own entry on the
    /// stack, and one that ran it again would repeat the discard
    /// that panicked.
    fn unwind_one(&mut self, scope: Scope) -> bool {
        if !self.tasks.scopes().contains(&scope) {
            return true;
        }
        if let Some(top) = self.tasks.pop_scope()
            && top != scope
        {
            self.discard_scope(top);
        }
        false
    }

    /// Give back the lends of a scope a failed call left behind and
    /// remove its record, without the checks the scope's own exit
    /// would have made: the call that would have made them is gone.
    /// The removal is the one the store's records define, so a
    /// subtask discarded here leaves its waitable set as any other
    /// removal would.
    ///
    /// Either kind of scope is discarded through the guard its own
    /// exit runs through, so that a scope discarded on the way to
    /// another one is as safe against a panic as the one being
    /// exited. The scope is off the stack already, so the guard's
    /// unwind step finds nothing to do.
    fn discard_scope(&mut self, scope: Scope) {
        match scope {
            Scope::Task(task) => {
                TaskExit::begin(self, task).finish();
            }
            Scope::Subtask(subtask) => {
                SubtaskExit::begin_discard(self, subtask).finish();
            }
        }
    }

    /// Restore the may-not-suspend flag of `task`'s instance to the
    /// value the enter intrinsic saved on the task's implicit thread.
    /// Does nothing for a task that never set the flag.
    fn restore_may_not_suspend(&mut self, task: TaskId) {
        let Some((instance, thread)) = self
            .tasks
            .task(task)
            .map(|record| (record.instance, record.implicit_thread))
        else {
            return;
        };
        let restore = self
            .tasks
            .thread_mut(thread)
            .and_then(|record| record.old_may_not_suspend.take());
        if let (Some(instance), Some(old)) = (instance, restore) {
            self.tasks.set_may_not_suspend(instance, old);
        }
    }

    /// Give back every owning entry lent to `scope`.
    fn undo_lends(&mut self, scope: Scope) {
        for (table, index) in self.tasks.take_lenders(scope) {
            if let Some(HandleKind::Own { lend_count, .. }) =
                self.for_table_mut(table).entry_mut(index)
            {
                *lend_count = lend_count.saturating_sub(1);
            }
        }
    }

    /// Record that a borrow of the owning entry `(table, index)` was
    /// lifted out during the current scope, so the entry cannot be
    /// removed until the scope ends. Fails when the index names no
    /// owning entry or when no scope is in flight to give the lend
    /// back.
    pub fn lend(&mut self, table: TableId, index: u32) -> Result<(), HandleLookupError> {
        self.lend_to(None, table, index)
    }

    /// Record the lend against the scope [`TaskTables::lending_scope`]
    /// reads off the stack, which is the record of the call in
    /// flight. A fused adapter's borrow transfer lends this way: the
    /// intrinsic is built once per instantiation and is handed no
    /// call of its own, so the stack is the only thing that names
    /// the call the caller is lending for.
    pub fn lend_for_call(&mut self, table: TableId, index: u32) -> Result<(), HandleLookupError> {
        let scope = self.tasks.lending_scope();
        self.lend_to(scope, table, index)
    }

    /// Record the lend against `scope` rather than against whatever
    /// is on top of the stack. A crossing names the scope its lends
    /// count against when it is built, and hands it here; `None`
    /// falls back to the current scope, for a caller that has no
    /// crossing of its own. The argument is read through
    /// [`TaskTables::counting_scope`], which is the one rule a
    /// crossing's scope is read by, here and in
    /// [`insert_borrow_for`](Self::insert_borrow_for).
    ///
    /// # Where a lend lives and when it comes back
    ///
    /// This is the one rule, and every crossing that lends obeys it:
    /// a handle the caller lends for a call goes on the record of
    /// that call, and comes back when the caller takes delivery of
    /// the call's result.
    ///
    /// - A guest calling a host function lends to the subtask the
    ///   trampoline pushed. The lend comes back when the guest takes
    ///   the subtask event, or as a synchronous lower returns.
    /// - A guest calling another component's export through a
    ///   prepared call lends to that call's subtask, not to the
    ///   callee's task. The two differ for a callback callee, which
    ///   can `task.return` and keep running: the caller's lend ends
    ///   at the delivery of the resolution, which is earlier than
    ///   the callee's task exit.
    /// - A guest calling another component's export through the
    ///   enter and exit intrinsics alone has no subtask record. The
    ///   callee's task is the record of that call, and its exit is
    ///   the call's return, so the lend goes there.
    /// - The host calling a guest export lends to the export's task,
    ///   and the lend comes back when that task resolves. That is
    ///   the return of `Func::call` and the resolution of the future
    ///   of `Func::call_concurrent`, so a callback callee that
    ///   returns and keeps running holds no host handle past its
    ///   `task.return`. [`resolve_task`](Self::resolve_task) is
    ///   where the two meet.
    /// - The host returning a `borrow<T>` out of a synchronous host
    ///   function the guest called lends to the caller's task. The
    ///   call's subtask has already left the stack by the time the
    ///   result crosses, so the scope the pop uncovered is the only
    ///   record left to lend against, and it is the right one: the
    ///   call is over, and what holds the borrow from here is the
    ///   task that made it.
    /// - The host returning a `borrow<T>` out of an asynchronous
    ///   host function lends to that call's subtask instead. That
    ///   lowering runs in the turn that resolves the subtask, while
    ///   the subtask is still the record of the call.
    ///
    /// # A host lend is counted like any other
    ///
    /// A handle the host lowers as a `borrow<T>` is lent on the same
    /// terms as one a guest lifts out of its own owning entry: the
    /// host's handle names an owning entry in the host's table for
    /// the resource type, that entry's count rises for the length of
    /// the crossing's scope, and the scope's end lowers it again.
    /// Nothing else about a host lend is special — the bullets above
    /// say where each one lives.
    ///
    /// While the lend stands, the entry cannot be taken back out of
    /// the host's table: `Store::resource_drop` from a host function
    /// the guest called, and a second lowering of the same handle as
    /// an `own<T>`, both fail with [`HandleLookupError::Lent`]. That
    /// is what makes the host's own handles obey the rule the
    /// canonical ABI states for every other lender, that an owning
    /// handle can be destroyed only while nothing is borrowing it.
    ///
    /// # A borrow entry is not counted as lent
    ///
    /// Only an owning entry can be lent: a borrow of a borrow is
    /// refused here with [`HandleLookupError::NotOwned`], and every
    /// call site skips the lend for such an entry rather than
    /// reaching it. A caller that drops a borrow entry while an
    /// asynchronous call still holds it is therefore not refused.
    ///
    /// This departs from the reference, whose `add_lender` counts
    /// any handle and whose `canon resource.drop` traps while the
    /// count is above zero. Wasmtime's `resource_lend` raises the
    /// count for an owning slot only and answers a borrow slot with
    /// the rep alone, and the polyfill follows Wasmtime: a borrow
    /// entry already belongs to the task it was lowered into and
    /// must be dropped before that task returns, so its own lifetime
    /// already bounds the window a lend would protect. Where the
    /// design documents are silent, Wasmtime's behavior decides.
    ///
    /// # Why the two writes are one step
    ///
    /// A lend is two writes — the count on the entry and the scope's
    /// list of lenders — and only the pair is safe: a count raised
    /// without a lender recorded is never given back, and the entry
    /// can never be removed again. Everything that can refuse the
    /// lend is therefore checked before either write happens.
    pub fn lend_to(
        &mut self,
        scope: Option<Scope>,
        table: TableId,
        index: u32,
    ) -> Result<(), HandleLookupError> {
        let Some(scope) = self.tasks.counting_scope(scope) else {
            return Err(HandleLookupError::NoCallInFlight);
        };
        match self.for_table(table).and_then(|t| t.entry(index)) {
            Some(HandleKind::Own { .. }) => {}
            Some(_) => return Err(HandleLookupError::NotOwned { index }),
            None => return Err(HandleLookupError::Unknown { index }),
        }
        if !self.tasks.add_lender(scope, (table, index)) {
            return Err(HandleLookupError::NoCallInFlight);
        }
        match self.for_table_mut(table).entry_mut(index) {
            Some(HandleKind::Own { lend_count, .. }) => *lend_count += 1,
            _ => unreachable!("the entry was an owning entry a moment ago"),
        }
        Ok(())
    }

    /// Insert an owning entry for `rep` of resource type `type_id`
    /// into `table` and return its index.
    pub fn insert_own(
        &mut self,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> u32 {
        self.for_table_mut(table).insert_entry(HandleKind::Own {
            type_id,
            guest_defined,
            rep,
            lend_count: 0,
        })
    }

    /// Insert a borrow of `rep` of resource type `type_id` into
    /// `table`, owed to the current task. Returns the new index, or
    /// `None` when no task is in flight.
    pub fn insert_borrow(
        &mut self,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> Option<u32> {
        self.insert_borrow_for(None, table, type_id, guest_defined, rep)
    }

    /// Insert the borrow against the scope `scope` names rather than
    /// against whatever is on top of the stack. A crossing names the
    /// scope its borrows count against when it is built, and hands
    /// it here; `None` falls back to the current scope, for a caller
    /// with no crossing of its own. The argument is read through
    /// [`TaskTables::counting_scope`], the same rule
    /// [`lend_to`](Self::lend_to) reads it by.
    ///
    /// A borrow is owed to a task, so the scope that came out of
    /// that rule is resolved to one through
    /// [`TaskTables::borrow_task`]: a subtask scope owes the borrow
    /// to the task that made the call, not to whatever task happens
    /// to be on top of the stack.
    pub fn insert_borrow_for(
        &mut self,
        scope: Option<Scope>,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> Option<u32> {
        let scope = self.tasks.counting_scope(scope)?;
        let task = self.tasks.borrow_task(scope)?;
        self.tasks.task_mut(task)?.num_borrows += 1;
        Some(self.for_table_mut(table).insert_entry(HandleKind::Borrow {
            type_id,
            guest_defined,
            rep,
            task,
        }))
    }

    /// Insert a subtask entry that names the subtask record
    /// `subtask`, and return the handle-table index.
    ///
    /// The record is told where it landed. The index is the first
    /// payload of every event the subtask delivers, and its presence
    /// is what says the caller has an entry to be told about at all.
    pub fn insert_subtask(&mut self, table: TableId, subtask: SubtaskId) -> u32 {
        let index = self
            .for_table_mut(table)
            .insert_entry(HandleKind::Subtask { subtask });
        self.tasks.set_subtask_handle(subtask, index);
        index
    }

    /// Insert a waitable-set entry that names the waitable set `set`,
    /// and return the handle-table index.
    pub fn insert_waitable_set(&mut self, table: TableId, set: WaitableSetId) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::WaitableSet { set })
    }

    /// Insert an entry of kind `kind` that names the end record
    /// `end`, and return the handle-table index.
    pub fn insert_end(&mut self, table: TableId, kind: EndKind, end: EndId) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::end(kind, end))
    }

    /// Read the entry at `index` of `table`, of any kind, with no
    /// type check. Used for a handle kind that carries no resource
    /// type, such as a subtask or a waitable set.
    pub fn entry(&self, table: TableId, index: u32) -> Option<HandleKind> {
        self.for_table(table).and_then(|t| t.entry(index)).copied()
    }

    /// Remove the entry at `index` of `table`, of any kind, with no
    /// type check and no ownership or borrow bookkeeping. A resource
    /// entry is removed through [`remove_own`](Self::remove_own)
    /// instead, which enforces that bookkeeping.
    pub fn remove(&mut self, table: TableId, index: u32) -> Option<HandleKind> {
        self.for_table_mut(table).remove(index)
    }

    /// Read the entry at `index` of `table`, checking that it holds a
    /// resource of type `type_id`. A component instance keeps one
    /// table for every handle kind it uses, so an index of one
    /// resource type can name an entry of another type, or an entry
    /// that is not a resource at all; both are the wrong-type trap
    /// and the wrong-kind failure respectively.
    pub fn lookup(
        &self,
        table: TableId,
        index: u32,
        type_id: ResourceTypeId,
        guest_defined: bool,
    ) -> Result<HandleKind, HandleLookupError> {
        let entry = self
            .for_table(table)
            .and_then(|t| t.entry(index))
            .copied()
            .ok_or(HandleLookupError::Unknown { index })?;
        let (found_type, found_guest) = match entry {
            HandleKind::Own {
                type_id,
                guest_defined,
                ..
            }
            | HandleKind::Borrow {
                type_id,
                guest_defined,
                ..
            } => (type_id, guest_defined),
            _ => return Err(HandleLookupError::WrongKind { index }),
        };
        if found_type != type_id {
            return Err(HandleLookupError::WrongType {
                index,
                expected_guest: guest_defined,
                found_guest,
            });
        }
        Ok(entry)
    }

    /// Remove the owning entry at `index` of `table` and return its
    /// rep. The entry must hold a resource of type `type_id`, must
    /// own it, and must not be lent out as a borrow.
    pub fn remove_own(
        &mut self,
        table: TableId,
        index: u32,
        type_id: ResourceTypeId,
        guest_defined: bool,
    ) -> Result<u32, HandleLookupError> {
        let entry = self.lookup(table, index, type_id, guest_defined)?;
        match entry {
            HandleKind::Own {
                lend_count: 0, rep, ..
            } => {
                self.for_table_mut(table).remove(index);
                Ok(rep)
            }
            HandleKind::Own { .. } => Err(HandleLookupError::Lent),
            HandleKind::Borrow { .. } => Err(HandleLookupError::NotOwned { index }),
            _ => unreachable!("lookup only ever returns a resource entry"),
        }
    }

    /// Drop a borrow entry: the guest returned the handle it was
    /// lent. Returns `false` when the task the borrow is owed to is
    /// no longer in the store.
    pub fn return_borrow(&mut self, task: TaskId) -> bool {
        match self.tasks.task_mut(task) {
            Some(record) => {
                record.num_borrows = record.num_borrows.saturating_sub(1);
                true
            }
            None => false,
        }
    }

    /// Borrow the table with the given identity, creating it on first
    /// access.
    pub fn for_table_mut(&mut self, table: TableId) -> &mut HandleTable {
        self.tables.entry(table).or_default()
    }

    /// Borrow the table with the given identity, or `None` if nothing
    /// has been allocated in it yet.
    pub fn for_table(&self, table: TableId) -> Option<&HandleTable> {
        self.tables.get(&table)
    }

    /// The host's table for a resource type, created on first access.
    /// Handles the host holds (`Store::resource_new`, an `own<T>`
    /// lifted out of a guest) live here.
    pub fn host_table(&mut self, type_id: ResourceTypeId) -> TableId {
        *self
            .host_tables
            .entry(type_id)
            .or_insert_with(TableId::fresh)
    }
}

impl Default for HandleTables {
    fn default() -> Self {
        Self::new()
    }
}

/// Panic inside the step named `step`, when a test has asked for a
/// panic there.
///
/// The steps of an exit guard are record operations over data the
/// store already holds, and none of them panics on its own; the
/// panic a guard exists for comes from below, from a runtime layer
/// or from an embedder's closure reached under the same lock. A
/// test cannot reach that, so it asks here instead, naming the step
/// it wants the panic in. The request is taken as it fires, so that
/// the drop which finishes the exit runs the steps that are left
/// rather than panicking in them too — which is the behaviour under
/// test, not a second thing to arrange.
#[cfg(test)]
fn panic_in_step(step: &'static str) {
    if PANIC_IN_STEP.with(|cell| cell.get()) == Some(step) {
        PANIC_IN_STEP.with(|cell| cell.set(None));
        panic!("the {step} step was asked to panic");
    }
}

#[cfg(test)]
thread_local! {
    /// The step an exit guard on this thread panics in, as a test
    /// asked.
    static PANIC_IN_STEP: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

/// Outside a test there is nothing to inject, and the call is gone.
#[cfg(not(test))]
fn panic_in_step(_step: &'static str) {}

/// The exit of one task's scope, in flight.
///
/// The exit is five record operations in sequence — the scope stack
/// unwinds to the task's own scope, the lends recorded against it go
/// back, the borrows it still owes are counted, the may-not-suspend
/// flag its enter intrinsic saved is restored, and its record is
/// removed — and only all five together leave the store consistent.
/// A panic between any two of them leaves a scope half unwound: an
/// entry still lent with nothing left to give the lend back, or a
/// record still there under a stack that no longer holds its scope.
/// The panic poisons the lock the exit ran under, and the guard a
/// turn holds clears that poison, so the half-unwound scope is what
/// every later reader of the store would see, with nothing left to
/// say it is half unwound.
///
/// The steps therefore run through this guard. It remembers which
/// of them are done, and runs the ones that are not when it is
/// dropped — which is what a panic anywhere in the sequence does to
/// it, during the unwind.
///
/// A step is taken before it runs: the guard moves past it first
/// and does the step's work second. That ordering is what makes the
/// finishing drop safe rather than merely intended. A guard that
/// moved on only after the step returned would find itself, during
/// the unwind, still pointing at the step the panic came from, and
/// would run that step again — a second panic while one is already
/// in flight, which aborts the process. The step a panic interrupts
/// is therefore lost, and the exit finishes without it; that is the
/// most an exit can do, because the panic left that one record
/// operation part-done and nothing records how much of it ran.
///
/// The unwind is the one step that is not taken that way, because
/// it is not one operation: it is a pop for every scope above the
/// task's own, and each pop discards a scope through a guard of its
/// own. What is taken there is the single pop, which happens before
/// the discard that can panic, and the guard stays on the unwind
/// step until the stack no longer holds the task's scope. A panic
/// in one of those discards therefore resumes at the next scope
/// down, rather than skipping an unwind that has the task's own
/// scope left to pop. Nothing in the step itself panics: what is
/// left of it once the discards are accounted for is a read of the
/// scope stack and a pop.
struct TaskExit<'a> {
    tables: &'a mut HandleTables,
    task: TaskId,
    /// The step to run next.
    step: ExitStep,
    /// The borrows the task still owed when the step that counts
    /// them ran, which is what the exit reports to its caller.
    borrows: u32,
}

/// Which step of a [`TaskExit`] runs next.
#[derive(Clone, Copy)]
enum ExitStep {
    Unwind,
    UndoLends,
    CountBorrows,
    RestoreMayNotSuspend,
    RemoveRecord,
    Done,
}

impl<'a> TaskExit<'a> {
    /// Begin the exit of `task`'s scope. A scope that is not on the
    /// stack leaves the unwind step with nothing to do, which is
    /// what the discard of a scope already popped wants.
    fn begin(tables: &'a mut HandleTables, task: TaskId) -> Self {
        Self {
            tables,
            task,
            step: ExitStep::Unwind,
            borrows: 0,
        }
    }

    /// Run the exit to its end and report the borrows the task left
    /// outstanding.
    fn finish(mut self) -> u32 {
        self.run();
        self.borrows
    }

    /// Run whichever steps have not run yet, in order. Running it
    /// twice is running it once: the second call starts at
    /// [`ExitStep::Done`] and does nothing.
    fn run(&mut self) {
        let scope = Scope::Task(self.task);
        loop {
            match self.step {
                ExitStep::Unwind => {
                    if self.tables.unwind_one(scope) {
                        self.step = ExitStep::UndoLends;
                    }
                }
                ExitStep::UndoLends => {
                    self.step = ExitStep::CountBorrows;
                    panic_in_step("task exit's undo-lends");
                    self.tables.undo_lends(scope);
                }
                ExitStep::CountBorrows => {
                    self.step = ExitStep::RestoreMayNotSuspend;
                    panic_in_step("task exit's count-borrows");
                    self.borrows = self
                        .tables
                        .tasks
                        .task(self.task)
                        .map(|record| record.num_borrows)
                        .unwrap_or(0);
                }
                ExitStep::RestoreMayNotSuspend => {
                    self.step = ExitStep::RemoveRecord;
                    panic_in_step("task exit's restore-may-not-suspend");
                    self.tables.restore_may_not_suspend(self.task);
                }
                ExitStep::RemoveRecord => {
                    self.step = ExitStep::Done;
                    panic_in_step("task exit's remove-record");
                    self.tables.tasks.remove_task(self.task);
                }
                ExitStep::Done => return,
            }
        }
    }
}

impl Drop for TaskExit<'_> {
    fn drop(&mut self) {
        self.run();
    }
}

/// The exit of one subtask's scope, in flight.
///
/// A subtask's exit has the shape a task's has, and it is guarded
/// for the reason [`TaskExit`]'s documentation gives: the steps are
/// only consistent together, a panic between any two of them leaves
/// the store half unwound, and the poison the panic left is cleared
/// by the turn that survives it. Each step is taken before it runs,
/// and the unwind is one pop at a time, exactly as there.
///
/// The guard serves the two ways a subtask's scope ends. A subtask
/// that resolved unwinds to its own scope, moves to the state its
/// resolution names, gives the caller back the handles it lent, and
/// leaves the store. A subtask a failed call left behind is
/// discarded instead: it is off the stack already and has no
/// resolution to deliver, so its lends go back unconditionally —
/// including any taken after a resolution was delivered, which the
/// delivery would not give back a second time — and its record is
/// removed.
struct SubtaskExit<'a> {
    tables: &'a mut HandleTables,
    subtask: SubtaskId,
    /// The state the resolution moves the subtask to, or `None` for
    /// a subtask being discarded, which has no resolution.
    state: Option<SubtaskState>,
    /// The step to run next.
    step: SubtaskExitStep,
}

/// Which step of a [`SubtaskExit`] runs next. The resolving exit
/// runs the unwind, the state, and the delivery before the removal;
/// the discard runs the undo-lends before it.
#[derive(Clone, Copy)]
enum SubtaskExitStep {
    Unwind,
    SetState,
    DeliverResolution,
    UndoLends,
    RemoveRecord,
    Done,
}

impl<'a> SubtaskExit<'a> {
    /// Begin the exit of `subtask`'s scope on its resolution to
    /// `state`.
    fn begin(tables: &'a mut HandleTables, subtask: SubtaskId, state: SubtaskState) -> Self {
        Self {
            tables,
            subtask,
            state: Some(state),
            step: SubtaskExitStep::Unwind,
        }
    }

    /// Begin the discard of `subtask`'s scope, which a failed call
    /// left behind: the scope is off the stack already and the call
    /// that would have resolved it is gone, so the exit starts at
    /// the lends.
    fn begin_discard(tables: &'a mut HandleTables, subtask: SubtaskId) -> Self {
        Self {
            tables,
            subtask,
            state: None,
            step: SubtaskExitStep::UndoLends,
        }
    }

    /// Run the exit to its end.
    fn finish(mut self) {
        self.run();
    }

    /// Run whichever steps have not run yet, in order. Running it
    /// twice is running it once: the second call starts at
    /// [`SubtaskExitStep::Done`] and does nothing.
    fn run(&mut self) {
        let scope = Scope::Subtask(self.subtask);
        loop {
            match self.step {
                SubtaskExitStep::Unwind => {
                    if self.tables.unwind_one(scope) {
                        self.step = SubtaskExitStep::SetState;
                    }
                }
                SubtaskExitStep::SetState => {
                    self.step = SubtaskExitStep::DeliverResolution;
                    panic_in_step("subtask exit's set-state");
                    if let (Some(state), Some(record)) =
                        (self.state, self.tables.tasks.subtask_mut(self.subtask))
                    {
                        record.state = state;
                    }
                }
                SubtaskExitStep::DeliverResolution => {
                    self.step = SubtaskExitStep::RemoveRecord;
                    panic_in_step("subtask exit's deliver-resolution");
                    self.tables.deliver_resolution(self.subtask);
                }
                SubtaskExitStep::UndoLends => {
                    self.step = SubtaskExitStep::RemoveRecord;
                    panic_in_step("subtask discard's undo-lends");
                    self.tables.undo_lends(scope);
                }
                SubtaskExitStep::RemoveRecord => {
                    self.step = SubtaskExitStep::Done;
                    panic_in_step("subtask exit's remove-record");
                    self.tables.tasks.remove_subtask(self.subtask);
                }
                SubtaskExitStep::Done => return,
            }
        }
    }
}

impl Drop for SubtaskExit<'_> {
    fn drop(&mut self) {
        self.run();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ResourceTypeIdInternal;

    #[wcmp_macros::test]
    fn it_rejects_an_index_of_another_resource_type() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let first = ResourceTypeId::fresh();
        let second = ResourceTypeId::fresh();
        let index = tables.insert_own(table, first, true, 7);
        assert_eq!(
            tables.lookup(table, index, second, true),
            Err(HandleLookupError::WrongType {
                index,
                expected_guest: true,
                found_guest: true,
            })
        );
        assert_eq!(
            tables
                .lookup(table, index, second, true)
                .unwrap_err()
                .to_string(),
            "handle index 1 used with the wrong type, expected guest-defined resource but found a different guest-defined resource"
        );
        assert_eq!(
            tables.lookup(table, index, first, true).unwrap().rep(),
            Some(7)
        );
    }

    #[wcmp_macros::test]
    fn it_allocates_consecutive_indices_across_resource_types_in_one_table() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let first = ResourceTypeId::fresh();
        let second = ResourceTypeId::fresh();
        let a = tables.insert_own(table, first, true, 1);
        let b = tables.insert_own(table, second, true, 2);
        assert_eq!(b, a + 1, "one allocator serves every resource type");
    }

    #[wcmp_macros::test]
    fn it_inserts_looks_up_and_removes_a_subtask_and_a_waitable_set_entry() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let subtask = tables.tasks.insert_subtask();
        let set = tables.tasks.insert_waitable_set();
        let subtask_index = tables.insert_subtask(table, subtask);
        let set_index = tables.insert_waitable_set(table, set);
        assert_ne!(subtask_index, set_index);
        assert_eq!(
            tables.entry(table, subtask_index),
            Some(HandleKind::Subtask { subtask })
        );
        assert_eq!(
            tables.entry(table, set_index),
            Some(HandleKind::WaitableSet { set })
        );

        let ty = ResourceTypeId::fresh();
        assert_eq!(
            tables.lookup(table, subtask_index, ty, true),
            Err(HandleLookupError::WrongKind {
                index: subtask_index
            })
        );
        assert_eq!(
            tables.lookup(table, set_index, ty, true),
            Err(HandleLookupError::WrongKind { index: set_index })
        );

        assert_eq!(
            tables.remove(table, subtask_index),
            Some(HandleKind::Subtask { subtask })
        );
        assert_eq!(
            tables.remove(table, set_index),
            Some(HandleKind::WaitableSet { set })
        );
        assert_eq!(tables.entry(table, subtask_index), None);
        assert_eq!(tables.entry(table, set_index), None);
    }

    #[wcmp_macros::test]
    fn it_refuses_to_remove_a_lent_entry_until_the_task_ends() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 3);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.lend(table, index), Ok(()));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Lent)
        );
        assert_eq!(tables.exit_task(task).borrows(), Ok(()));
        assert_eq!(tables.remove_own(table, index, ty, false), Ok(3));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Unknown { index })
        );
    }

    #[wcmp_macros::test]
    fn it_owes_a_lowered_borrow_to_the_current_task_and_takes_it_back_on_drop() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        let index = tables
            .insert_borrow(table, ty, false, 11)
            .expect("a task is in flight");
        tables
            .insert_borrow(table, ty, false, 12)
            .expect("a task is in flight");
        assert_eq!(
            tables.entry(table, index),
            Some(HandleKind::Borrow {
                type_id: ty,
                guest_defined: false,
                rep: 11,
                task,
            })
        );

        assert!(
            tables.return_borrow(task),
            "the guest drops one of the two borrows"
        );
        assert_eq!(
            tables.exit_task(task).borrows(),
            Err(1),
            "the drop took one back and the guest still holds the other"
        );
        assert!(
            !tables.return_borrow(task),
            "the task is gone once its scope ended"
        );

        // A task that sees every borrow it was lowered dropped owes
        // nothing at its exit.
        let task = tables.tasks.push_task(None, None, instance);
        tables
            .insert_borrow(table, ty, false, 13)
            .expect("a task is in flight");
        assert!(tables.return_borrow(task), "the guest drops the borrow");
        assert_eq!(
            tables.exit_task(task).borrows(),
            Ok(()),
            "nothing is owed when the count is back to zero"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_borrow_lowered_outside_a_task() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        assert_eq!(tables.insert_borrow(table, ty, false, 1), None);
    }

    #[wcmp_macros::test]
    fn it_discards_a_subtask_a_failed_host_call_left_above_the_task() {
        // The failure of a host call travels past the pop that would
        // have ended its subtask: the lift of a parameter fails after
        // an earlier parameter has already lent an owning entry. The
        // task that catches the failure ends the subtask with itself.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 4);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        let subtask = tables.tasks.push_subtask();
        assert_eq!(tables.lend(table, index), Ok(()), "the borrow lifts out");
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Lent),
            "the entry is lent while the host call runs"
        );

        tables.abandon_task(task);

        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(4),
            "the lend the abandoned subtask held is given back"
        );
        assert!(
            tables.tasks.scopes().is_empty(),
            "neither scope is left on the stack"
        );
        assert_eq!(tables.tasks.subtask(subtask).map(|_| ()), None);
        assert_eq!(tables.tasks.task_count(), 0, "no task record is left");
        assert_eq!(tables.tasks.subtask_count(), 0, "no subtask record either");
        assert_eq!(tables.tasks.thread_count(), 0, "nor any thread record");
    }

    #[wcmp_macros::test]
    fn it_leaves_a_scope_that_is_not_on_the_stack_alone() {
        // An identity that has already been popped names nothing, and
        // ending it a second time must not eat the scope below it.
        let mut tables = HandleTables::new();
        let instance = tables.tasks.insert_instance();
        let outer = tables.tasks.push_task(None, None, instance);
        let inner = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.exit_task(inner), TaskEnd::Ended(0));

        assert_eq!(
            tables.exit_task(inner),
            TaskEnd::Untouched,
            "the second exit is a no-op, and says so, because its caller \
             has a second half to run only for a task it really ended"
        );

        assert_eq!(
            tables.tasks.current_scope(),
            Some(Scope::Task(outer)),
            "the caller's task is still current"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_a_borrow_left_by_a_failed_call_from_reaching_a_later_task() {
        // A call that fails with a borrow outstanding leaves the
        // borrow entry in the guest's table, naming a task whose
        // record is gone. The next call takes the freed record index,
        // so the stale entry must not name it.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let instance = tables.tasks.insert_instance();

        let failed = tables.tasks.push_task(None, None, instance);
        let stale_entry = tables
            .insert_borrow(table, ty, false, 21)
            .expect("a task is in flight");
        tables.abandon_task(failed);

        let later = tables.tasks.push_task(None, None, instance);
        assert_eq!(
            later.index(),
            failed.index(),
            "the record table hands the freed index out again"
        );
        assert_ne!(later, failed, "the generation tells the two calls apart");
        tables
            .insert_borrow(table, ty, false, 22)
            .expect("a task is in flight");

        let Some(HandleKind::Borrow { task: stale, .. }) = tables.entry(table, stale_entry) else {
            panic!("the failed call's borrow entry is still in the table");
        };
        assert_eq!(stale, failed, "the entry still names the call that failed");
        assert!(
            !tables.return_borrow(stale),
            "the failed call's identity names no record"
        );
        assert_eq!(
            tables.exit_task(later).borrows(),
            Err(1),
            "the later call still owes the one borrow it was lowered"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_lend_to_a_subtask_scope_that_has_already_ended() {
        // A crossing names the scope its lends count against when it
        // is built. A crossing built for a call out that then failed
        // names a subtask whose record is gone, and the next call out
        // takes the freed index: the lend must be refused, not
        // recorded against the later call.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 4);

        let failed = tables.tasks.push_subtask();
        tables.abandon_subtask(failed);
        let later = tables.tasks.push_subtask();
        assert_eq!(
            later.index(),
            failed.index(),
            "the record table hands the freed index out again"
        );

        assert_eq!(
            tables.lend_to(Some(Scope::Subtask(failed)), table, index),
            Err(HandleLookupError::NoCallInFlight),
            "the scope the crossing named has already ended"
        );
        assert!(
            tables
                .tasks
                .subtask(later)
                .expect("the later call")
                .lenders
                .is_empty(),
            "nothing was recorded against the call that took the index"
        );
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(4),
            "and the refused lend left the entry free to be removed"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_lend_with_no_call_to_give_it_back() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 5);

        assert_eq!(
            tables.lend(table, index),
            Err(HandleLookupError::NoCallInFlight),
            "nothing is in flight to record the lend against"
        );
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(5),
            "the refused lend left the count untouched"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_lend_to_a_scope_that_has_already_ended() {
        // A crossing carries the scope its lends count against. A
        // failure can end that scope before the crossing lifts its
        // last argument, and a lend recorded against a record that is
        // gone would never be given back.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 6);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.exit_task(task).borrows(), Ok(()));

        assert_eq!(
            tables.lend_to(Some(Scope::Task(task)), table, index),
            Err(HandleLookupError::NoCallInFlight),
            "the named scope's record is gone"
        );
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(6),
            "the refused lend left the count untouched"
        );
    }

    #[wcmp_macros::test]
    fn it_reads_a_subtask_scope_the_same_way_for_a_lend_and_for_a_borrow() {
        // A crossing hands the same scope to both operations, so both
        // read it by the same rule: the named subtask is the call the
        // lend is given back to, and the task that made that call is
        // the one the borrow is owed to. Neither consults the top of
        // the stack instead, which here is a second, unrelated task
        // running while the host side of the call runs.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let owned = tables.insert_own(table, ty, false, 4);
        let instance = tables.tasks.insert_instance();
        let caller = tables.tasks.push_task(None, None, instance);
        let subtask = tables.tasks.push_subtask();
        let nested = tables.tasks.push_task(None, None, instance);

        let scope = Some(Scope::Subtask(subtask));
        assert_eq!(tables.lend_to(scope, table, owned), Ok(()));
        let borrow = tables
            .insert_borrow_for(scope, table, ty, false, 5)
            .expect("the named subtask names the call the borrow is owed to");

        assert_eq!(
            tables.entry(table, borrow),
            Some(HandleKind::Borrow {
                type_id: ty,
                guest_defined: false,
                rep: 5,
                task: caller,
            }),
            "the borrow is owed to the task that made the call, not to the task on the stack"
        );
        assert_eq!(
            tables.exit_task(nested).borrows(),
            Ok(()),
            "the task on top of the stack was handed neither the lend nor the borrow"
        );
        assert_eq!(
            tables.remove_own(table, owned, ty, false),
            Err(HandleLookupError::Lent),
            "the lend is held by the named subtask, which has not resolved"
        );

        tables.exit_subtask(subtask, SubtaskState::Returned);
        assert_eq!(
            tables.remove_own(table, owned, ty, false),
            Ok(4),
            "resolving the call gave the lend back"
        );
        assert_eq!(
            tables.exit_task(caller).borrows(),
            Err(1),
            "and the borrow is still owed to the task that made the call"
        );
    }

    /// Run `body` and catch the panic it is expected to unwind
    /// with, keeping the report of that panic out of the test's
    /// output.
    #[cfg(not(target_arch = "wasm32"))]
    fn unwind<R>(body: impl FnOnce() -> R) -> std::thread::Result<R> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        std::panic::set_hook(hook);
        outcome
    }

    /// Ask the next exit guard on this thread to panic inside the
    /// step named `step`, which is how a test puts a panic where an
    /// exit's own steps never put one.
    #[cfg(not(target_arch = "wasm32"))]
    fn panic_in(step: &'static str) {
        PANIC_IN_STEP.with(|cell| cell.set(Some(step)));
    }

    // The browser target aborts on a panic instead of unwinding, so
    // there is nothing to catch there and these tests are native
    // only.

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_finishes_a_task_exit_a_panic_inside_a_step_interrupted() {
        // A task with a subtask above it that a failed host call
        // left behind, and an owning entry lent to the task itself:
        // the exit has a scope to discard, a lend to give back, and
        // two records to remove.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 8);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.lend(table, index), Ok(()), "the borrow lifts out");
        let subtask = tables.tasks.push_subtask();

        // The panic is inside a step, which is where the double
        // panic used to come from: the guard had not moved past the
        // step it was running, so the drop that finishes the exit
        // ran that step a second time, during the unwind.
        panic_in("task exit's undo-lends");
        let unwound = unwind(|| {
            let _ = tables.exit_task(task).borrows();
        });
        assert!(
            unwound.is_err(),
            "the panic unwound past the exit, once, rather than aborting"
        );

        assert!(
            tables.tasks.scopes().is_empty(),
            "the later reader finds neither the task's scope nor the subtask \
             above it on the stack"
        );
        assert!(
            tables.tasks.task(task).is_none(),
            "the task's record left the store with its scope"
        );
        assert!(
            tables.tasks.subtask(subtask).is_none(),
            "so did the subtask's"
        );
        assert_eq!(tables.tasks.thread_count(), 0, "nor is a thread left");
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Lent),
            "the lend the interrupted step was giving back is what the panic \
             costs: the step is taken before it runs, so the exit finishes \
             without it rather than running it again"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_runs_the_later_steps_of_a_task_exit_a_panic_interrupted() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 8);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.lend(table, index), Ok(()), "the borrow lifts out");

        // A panic in a step past the lends: every step before it
        // stands, and every step after it runs during the unwind.
        panic_in("task exit's restore-may-not-suspend");
        let unwound = unwind(|| {
            let _ = tables.exit_task(task).borrows();
        });
        assert!(unwound.is_err(), "the panic unwound past the exit");

        assert!(
            tables.tasks.scopes().is_empty(),
            "the scope is off the stack"
        );
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(8),
            "the lend the step before the panic gave back is back"
        );
        assert!(
            tables.tasks.task(task).is_none(),
            "and the record the step after it removes is gone"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_finishes_a_subtask_exit_a_panic_inside_a_step_interrupted() {
        // A subtask on top of its caller's task, with an owning
        // entry lent to the call it made.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 4);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        let subtask = tables.tasks.push_subtask();
        assert_eq!(tables.lend(table, index), Ok(()), "the borrow lifts out");

        panic_in("subtask exit's set-state");
        let unwound = unwind(|| {
            tables.exit_subtask(subtask, SubtaskState::Returned);
        });
        assert!(
            unwound.is_err(),
            "the panic unwound past the exit, once, rather than aborting"
        );

        assert_eq!(
            tables.tasks.scopes(),
            &[Scope::Task(task)],
            "the subtask's scope is off the stack and its caller's is not"
        );
        assert!(
            tables.tasks.subtask(subtask).is_none(),
            "the subtask's record left the store with its scope"
        );
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Ok(4),
            "and the resolution the steps after the panic delivered gave the \
             caller its lend back"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn it_finishes_a_task_exit_a_panic_inside_a_discarded_scope_interrupted() {
        // The same shape as the task exit's own test, but the panic
        // is in a step of the discard the unwind runs, which is a
        // guard inside the guard.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 8);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        let subtask = tables.tasks.push_subtask();
        assert_eq!(tables.lend(table, index), Ok(()), "the borrow lifts out");

        panic_in("subtask discard's undo-lends");
        let unwound = unwind(|| {
            let _ = tables.exit_task(task).borrows();
        });
        assert!(
            unwound.is_err(),
            "the panic unwound past both guards, once, rather than aborting"
        );

        assert!(
            tables.tasks.subtask(subtask).is_none(),
            "the discard finished its own remaining step"
        );
        assert!(
            tables.tasks.scopes().is_empty(),
            "and the exit resumed its unwind at the scope below, so the \
             task's own scope is not stranded on the stack"
        );
        assert!(
            tables.tasks.task(task).is_none(),
            "the task's record left with it"
        );
        assert_eq!(tables.tasks.thread_count(), 0, "nor is a thread left");
    }

    #[wcmp_macros::test]
    fn it_refuses_a_lend_of_an_entry_that_owns_nothing() {
        // A borrow entry is not counted as lent, which is the
        // departure from the reference that [`HandleTables::lend_to`]
        // states: the reference counts any handle, Wasmtime counts an
        // owning slot alone, and the polyfill follows Wasmtime.
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let instance = tables.tasks.insert_instance();
        tables.tasks.push_task(None, None, instance);
        let borrow = tables
            .insert_borrow(table, ty, false, 7)
            .expect("a task is in flight");

        assert_eq!(
            tables.lend(table, borrow),
            Err(HandleLookupError::NotOwned { index: borrow })
        );
        assert_eq!(
            tables.lend(table, borrow + 1),
            Err(HandleLookupError::Unknown { index: borrow + 1 })
        );
    }
}
