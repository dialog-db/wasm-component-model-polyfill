//! A call the polyfill itself makes into a guest.
//!
//! Three calls cross from the polyfill into a guest without the
//! guest having asked for anything. The first is the `cabi_realloc` a
//! crossing asks for memory with. The second is the `post-return` an
//! export runs once the caller has observed the return value. The
//! third is the destructor a `resource.drop` runs, whether a guest
//! dropped the last owning handle or the host released one of its
//! own. None of the three is a call into an export, and none is the
//! guest calling anything: the polyfill drives them all.
//!
//! The reference lifts `realloc` as a function and invokes it, so
//! each realloc call is a task with one thread. The thread starts
//! with zero context slots and its slots end with it, so a slot the
//! realloc sets reaches neither the export that runs next nor the
//! task that made the host call whose result is being lowered. The
//! reference lifts a destructor the same way, as a synchronous
//! function of one `u32` parameter, so a destructor is a task with
//! one thread under the same rule: it sees zeros, and what it sets
//! does not reach the thread that dropped the handle.
//!
//! The reference clears the instance's may-leave flag around the
//! realloc and the post-return, so a built-in that reads the flag
//! traps with the cannot-leave cause for as long as one of them
//! runs. It does not clear it around a destructor.
//!
//! [`BoundaryCall`] is that entry, held for the length of the call.
//! Its drop undoes whichever halves the entry did, so a call that
//! failed or trapped leaves neither a task on the stack nor the flag
//! clear.

use std::sync::{Arc, Mutex};

use crate::abi::instance::BoundaryInstance;
use crate::concurrency::{InstanceId, TaskId};
use crate::error::{Error, Result};
use crate::resource::HandleTables;

/// One call the polyfill makes into a guest, in flight.
///
/// The value is a guard. Build it immediately before the call and
/// drop it immediately after, whether the call returned or failed.
pub struct BoundaryCall {
    /// The store's records, through which the drop undoes what the
    /// entry did. `None` for a crossing that reaches no records,
    /// which is a copy between two guest memories.
    tables: Option<Arc<Mutex<HandleTables>>>,
    /// The instance whose may-leave flag the entry cleared, with the
    /// value the drop gives back.
    may_leave: Option<(InstanceId, bool)>,
    /// The task the call runs as, when it runs as one.
    task: Option<TaskId>,
}

impl BoundaryCall {
    /// Enter a call into the `cabi_realloc` of `instance`: a task
    /// with one fresh thread becomes the current scope, and the
    /// instance may not be left until the call ends.
    pub fn realloc(instance: &BoundaryInstance) -> Result<Self> {
        Self::enter(instance, true)
    }

    /// Enter a call into the `post-return` of an export of
    /// `instance`: the instance may not be left until the call ends.
    /// The reference calls the `post-return` from the export's own
    /// task, so the call pushes no task of its own.
    pub fn post_return(instance: &BoundaryInstance) -> Result<Self> {
        Self::enter(instance, false)
    }

    /// Enter a call into the destructor of a resource: a task with
    /// one fresh thread becomes the current scope. `instance` is the
    /// component instance that implements the resource, and `None`
    /// when the host does. The instance may still be left, because
    /// the reference clears the flag around a realloc and a
    /// post-return and not around a destructor.
    pub fn destructor(
        tables: &Arc<Mutex<HandleTables>>,
        instance: Option<InstanceId>,
    ) -> Result<Self> {
        let mut call = Self {
            tables: None,
            may_leave: None,
            task: None,
        };
        let mut guard = tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let task = match instance {
            Some(instance) => guard.tasks.push_task(None, None, instance),
            None => guard.tasks.push_task_without_instance(),
        };
        guard.tasks.start_task(task);
        drop(guard);
        call.task = Some(task);
        call.tables = Some(tables.clone());
        Ok(call)
    }

    /// Enter the call, pushing a task of its own when `as_task`.
    fn enter(instance: &BoundaryInstance, as_task: bool) -> Result<Self> {
        let mut call = Self {
            tables: None,
            may_leave: None,
            task: None,
        };
        let (Some(tables), Some(id)) = (instance.tables(), instance.id()) else {
            return Ok(call);
        };
        let mut guard = tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        if as_task {
            let task = guard.tasks.push_task(None, None, id);
            guard.tasks.start_task(task);
            call.task = Some(task);
        }
        call.may_leave = guard.tasks.set_may_leave(id, false).map(|old| (id, old));
        drop(guard);
        call.tables = Some(tables.clone());
        Ok(call)
    }
}

impl Drop for BoundaryCall {
    /// End the call: the task and its thread leave the store, and
    /// the instance may be left again. A poisoned lock leaves both
    /// undone, because there is no record left to read or write.
    fn drop(&mut self) {
        let Some(tables) = self.tables.take() else {
            return;
        };
        let Ok(mut guard) = tables.lock() else {
            return;
        };
        if let Some(task) = self.task.take() {
            // The scope is popped, the record and its thread are
            // removed, and a scope a trapping call left above it
            // goes with them; what an exit would add, the check
            // that the task holds no borrow as it ends, is skipped.
            // A realloc and a post-return carry no borrow of their
            // own, so for them there is nothing to check. A
            // destructor is arbitrary guest code: it can call an
            // import and receive a borrow lowered into its own
            // task, and the reference traps when such a task
            // returns holding one. No directive of the corpus
            // reaches that, and checking here would need a cause
            // and a test of its own, so the count stays unchecked.
            guard.abandon_task(task);
        }
        if let Some((instance, old)) = self.may_leave.take() {
            guard.tasks.set_may_leave(instance, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::concurrency::Scope;
    use crate::executor::ir::{CanonOptions, DataModel, StringEncoding};
    use crate::resource::TableId;

    /// A store's records with one component instance in them, the
    /// instance's identity, and the crossing that instance's options
    /// resolve to.
    fn records() -> (Arc<Mutex<HandleTables>>, InstanceId, BoundaryInstance) {
        let mut handles = HandleTables::new();
        let instance = handles.tasks.insert_instance();
        let tables = Arc::new(Mutex::new(handles));
        let declared = CanonOptions {
            instance: 0,
            memory: None,
            realloc: None,
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        };
        let state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
            0,
            0,
            0,
            0,
            Vec::new(),
            vec![instance],
            vec![TableId::fresh()],
        )));
        let (_, boundary) =
            BoundaryInstance::resolve(&declared, &state, &tables).expect("resolve the crossing");
        (tables, instance, boundary)
    }

    /// Whether the instance may be left, and how many tasks, threads,
    /// and scopes the store holds.
    fn state(
        tables: &Arc<Mutex<HandleTables>>,
        instance: InstanceId,
    ) -> (bool, usize, usize, usize) {
        let guard = tables.lock().expect("handle tables");
        (
            guard
                .tasks
                .instance(instance)
                .expect("instance record")
                .may_leave,
            guard.tasks.task_count(),
            guard.tasks.thread_count(),
            guard.tasks.scopes().len(),
        )
    }

    #[wcmp_macros::test]
    fn it_runs_a_realloc_as_a_task_with_one_thread_that_may_not_leave() {
        let (tables, instance, boundary) = records();
        assert_eq!(state(&tables, instance), (true, 0, 0, 0));

        let call = BoundaryCall::realloc(&boundary).expect("enter the realloc");
        let (may_leave, tasks, threads, scopes) = state(&tables, instance);
        assert!(
            !may_leave,
            "the instance may not be left while a realloc runs"
        );
        assert_eq!((tasks, threads, scopes), (1, 1, 1), "one task, one thread");
        {
            let guard = tables.lock().expect("handle tables");
            let task = guard.tasks.current_task().expect("the realloc's task");
            assert_eq!(
                guard.tasks.current_scope(),
                Some(Scope::Task(task)),
                "the realloc's task is the current scope"
            );
            let thread = guard.tasks.current_thread().expect("the realloc's thread");
            assert_eq!(
                guard.tasks.thread(thread).expect("thread record").context,
                [0, 0],
                "the fresh thread starts with zero context slots"
            );
        }

        drop(call);
        assert_eq!(
            state(&tables, instance),
            (true, 0, 0, 0),
            "the task, its thread, and the cleared flag all end with the call"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_a_post_return_inside_the_task_that_called_the_export() {
        let (tables, instance, boundary) = records();
        let export = tables
            .lock()
            .expect("handle tables")
            .tasks
            .push_task(None, None, instance);

        let call = BoundaryCall::post_return(&boundary).expect("enter the post-return");
        let (may_leave, tasks, threads, scopes) = state(&tables, instance);
        assert!(
            !may_leave,
            "the instance may not be left while a post-return runs"
        );
        assert_eq!(
            (tasks, threads, scopes),
            (1, 1, 1),
            "the post-return pushes no task of its own"
        );
        assert_eq!(
            tables.lock().expect("handle tables").tasks.current_scope(),
            Some(Scope::Task(export)),
            "the export's own task is still the current scope"
        );

        drop(call);
        assert_eq!(
            state(&tables, instance),
            (true, 1, 1, 1),
            "the flag is given back and the export's task is untouched"
        );
    }

    #[wcmp_macros::test]
    fn it_ends_the_call_when_the_call_fails() {
        // A realloc that traps unwinds past the site that would have
        // dropped the guard by hand, so the guard's own drop is what
        // ends the call.
        let (tables, instance, boundary) = records();
        let failed: std::result::Result<(), ()> = {
            let _call = BoundaryCall::realloc(&boundary).expect("enter the realloc");
            Err(())
        };
        assert!(failed.is_err());
        assert_eq!(
            state(&tables, instance),
            (true, 0, 0, 0),
            "nothing of the failed call is left in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_does_nothing_for_a_crossing_that_reaches_no_records() {
        // A copy between two guest memories names no tables, so
        // there is nothing to push a task into.
        let call = BoundaryCall::realloc(&BoundaryInstance::without_tables(None))
            .expect("enter the realloc");
        drop(call);
    }
}
