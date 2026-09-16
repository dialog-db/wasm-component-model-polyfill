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
//! the current scope, and delivering a subtask's event lowers those
//! counts again.
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
    Event, Scope, SubtaskId, SubtaskState, TaskId, TaskTables, ThreadId, WaitableId, WaitableSetId,
};
use crate::error::Error;

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
}

impl HandleTables {
    /// Construct an empty collection.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            host_tables: HashMap::new(),
            tasks: TaskTables::new(),
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
    /// on the stack.
    pub fn exit_task(&mut self, task: TaskId) -> Result<(), u32> {
        if !self.unwind_to(Scope::Task(task)) {
            return Ok(());
        }
        self.undo_lends(Scope::Task(task));
        let borrows = self
            .tasks
            .task(task)
            .map(|record| record.num_borrows)
            .unwrap_or(0);
        self.restore_may_not_suspend(task);
        self.tasks.remove_task(task);
        if borrows > 0 { Err(borrows) } else { Ok(()) }
    }

    /// End the innermost task on the stack on its success path, for
    /// the one caller that cannot name the task it pushed: an
    /// adapter's enter and exit intrinsics are two separate calls
    /// that pass no identity between them. Behaves as
    /// [`exit_task`](Self::exit_task) in every other respect.
    pub fn exit_current_task(&mut self) -> Result<(), u32> {
        match self.tasks.current_task() {
            Some(task) => self.exit_task(task),
            None => Ok(()),
        }
    }

    /// End the task `task` on its failure path: the scope is popped
    /// and its lends undone, and no borrow check is made, because
    /// the call already failed. Every scope the failure left above
    /// `task` is discarded with it.
    pub fn abandon_task(&mut self, task: TaskId) {
        let _ = self.exit_task(task);
    }

    /// Deliver the subtask `subtask`'s resolution and pop it: the
    /// subtask moves to `state`, the count on each handle it
    /// borrowed is decremented, and its record is removed. A scope a
    /// failed call left above it is discarded first, and nothing
    /// happens when `subtask` is not on the stack.
    pub fn exit_subtask(&mut self, subtask: SubtaskId, state: SubtaskState) {
        if !self.unwind_to(Scope::Subtask(subtask)) {
            return;
        }
        if let Some(record) = self.tasks.subtask_mut(subtask) {
            record.state = state;
        }
        self.deliver_resolution(subtask);
        self.tasks.remove_subtask(subtask);
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
            Some(HandleKind::Subtask { index }) => {
                Ok(self.tasks.subtask_waitable(SubtaskId::from_index(index)))
            }
            Some(_) => Err(HandleLookupError::WrongKind { index }),
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
            Some(HandleKind::WaitableSet { index }) => Ok(WaitableSetId::from_index(index)),
            Some(_) => Err(HandleLookupError::WrongKind { index }),
            None => Err(HandleLookupError::Unknown { index }),
        }
    }

    /// Take the event pending on `waitable`, leaving its slot empty.
    /// Taking a subtask's event also delivers the subtask's
    /// resolution when the call has resolved, which is what gives the
    /// handles it borrowed back to the caller.
    fn take_event(&mut self, waitable: WaitableId) -> Result<Option<Event>, Error> {
        let event = self.tasks.take_pending_event(waitable)?;
        if event.is_some()
            && let WaitableId::Subtask(subtask) = waitable
        {
            let resolved = self
                .tasks
                .subtask(subtask)
                .map(|record| record.state.resolved())
                .unwrap_or(false);
            if resolved {
                self.deliver_resolution(subtask);
            }
        }
        Ok(event)
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

    /// Pop scopes until `scope` itself has been popped, discarding
    /// every scope above it. Those are what a call that failed
    /// between its own push and its own pop left behind: the failure
    /// travels as an error or a trap past the pop that would have
    /// ended the scope, so the scope that catches it ends them.
    /// Returns `false`, having popped nothing, when `scope` is not on
    /// the stack.
    fn unwind_to(&mut self, scope: Scope) -> bool {
        if !self.tasks.scopes().contains(&scope) {
            return false;
        }
        while let Some(top) = self.tasks.pop_scope() {
            if top == scope {
                return true;
            }
            self.discard_scope(top);
        }
        false
    }

    /// Give back the lends of a scope a failed call left behind and
    /// remove its record, without the checks the scope's own exit
    /// would have made: the call that would have made them is gone.
    fn discard_scope(&mut self, scope: Scope) {
        self.undo_lends(scope);
        match scope {
            Scope::Task(task) => {
                self.restore_may_not_suspend(task);
                self.tasks.remove_task(task);
            }
            Scope::Subtask(subtask) => {
                self.tasks.remove_subtask(subtask);
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
        if let Some(old) = restore {
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
    /// removed until the scope ends. Returns `false` when the entry
    /// is not an owning entry or no scope is in flight.
    pub fn lend(&mut self, table: TableId, index: u32) -> bool {
        self.lend_to(None, table, index)
    }

    /// Record the lend against `scope` rather than against whatever
    /// is on top of the stack. A crossing names the scope its lends
    /// count against when it is built, and hands it here; `None`
    /// falls back to the current scope, for a caller that has no
    /// crossing of its own.
    pub fn lend_to(&mut self, scope: Option<Scope>, table: TableId, index: u32) -> bool {
        let Some(scope) = scope.or_else(|| self.tasks.current_scope()) else {
            return false;
        };
        match self.for_table_mut(table).entry_mut(index) {
            Some(HandleKind::Own { lend_count, .. }) => {
                *lend_count += 1;
                self.tasks.add_lender(scope, (table, index))
            }
            _ => false,
        }
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

    /// Insert the borrow against the task `scope` names rather than
    /// against whatever is on top of the stack. A crossing names the
    /// scope its borrows count against when it is built, and hands
    /// it here. A crossing counted against a subtask, and a caller
    /// with no crossing of its own, fall back to the innermost task
    /// on the stack, which is the task the borrow is owed to either
    /// way.
    pub fn insert_borrow_for(
        &mut self,
        scope: Option<Scope>,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> Option<u32> {
        let task = match scope {
            Some(Scope::Task(task)) => task,
            _ => self.tasks.current_task()?,
        };
        self.tasks.task_mut(task)?.num_borrows += 1;
        Some(self.for_table_mut(table).insert_entry(HandleKind::Borrow {
            type_id,
            guest_defined,
            rep,
            task,
        }))
    }

    /// Insert a subtask entry that points at index `subtask` of the
    /// store's subtask table, and return the handle-table index.
    pub fn insert_subtask(&mut self, table: TableId, subtask: u32) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::Subtask { index: subtask })
    }

    /// Insert a waitable-set entry that points at index `set` of the
    /// store's waitable-set table, and return the handle-table index.
    pub fn insert_waitable_set(&mut self, table: TableId, set: u32) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::WaitableSet { index: set })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    #[test]
    fn it_allocates_consecutive_indices_across_resource_types_in_one_table() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let first = ResourceTypeId::fresh();
        let second = ResourceTypeId::fresh();
        let a = tables.insert_own(table, first, true, 1);
        let b = tables.insert_own(table, second, true, 2);
        assert_eq!(b, a + 1, "one allocator serves every resource type");
    }

    #[test]
    fn it_inserts_looks_up_and_removes_a_subtask_and_a_waitable_set_entry() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let subtask_index = tables.insert_subtask(table, 5);
        let set_index = tables.insert_waitable_set(table, 9);
        assert_ne!(subtask_index, set_index);
        assert_eq!(
            tables.entry(table, subtask_index),
            Some(HandleKind::Subtask { index: 5 })
        );
        assert_eq!(
            tables.entry(table, set_index),
            Some(HandleKind::WaitableSet { index: 9 })
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
            Some(HandleKind::Subtask { index: 5 })
        );
        assert_eq!(
            tables.remove(table, set_index),
            Some(HandleKind::WaitableSet { index: 9 })
        );
        assert_eq!(tables.entry(table, subtask_index), None);
        assert_eq!(tables.entry(table, set_index), None);
    }

    #[test]
    fn it_refuses_to_remove_a_lent_entry_until_the_task_ends() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 3);
        let instance = tables.tasks.insert_instance();
        let task = tables.tasks.push_task(None, None, instance);
        assert!(tables.lend(table, index));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Lent)
        );
        assert_eq!(tables.exit_task(task), Ok(()));
        assert_eq!(tables.remove_own(table, index, ty, false), Ok(3));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Unknown { index })
        );
    }

    #[test]
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
            tables.exit_task(task),
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
            tables.exit_task(task),
            Ok(()),
            "nothing is owed when the count is back to zero"
        );
    }

    #[test]
    fn it_refuses_a_borrow_lowered_outside_a_task() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        assert_eq!(tables.insert_borrow(table, ty, false, 1), None);
    }

    #[test]
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
        assert!(tables.lend(table, index), "the borrow lifts out");
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

    #[test]
    fn it_leaves_a_scope_that_is_not_on_the_stack_alone() {
        // An identity that has already been popped names nothing, and
        // ending it a second time must not eat the scope below it.
        let mut tables = HandleTables::new();
        let instance = tables.tasks.insert_instance();
        let outer = tables.tasks.push_task(None, None, instance);
        let inner = tables.tasks.push_task(None, None, instance);
        assert_eq!(tables.exit_task(inner), Ok(()));

        assert_eq!(
            tables.exit_task(inner),
            Ok(()),
            "the second exit is a no-op"
        );

        assert_eq!(
            tables.tasks.current_scope(),
            Some(Scope::Task(outer)),
            "the caller's task is still current"
        );
    }
}
