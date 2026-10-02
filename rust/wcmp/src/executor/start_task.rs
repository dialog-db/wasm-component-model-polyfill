// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The task a core module's `start` function runs in.
//!
//! Instantiating a core module runs whatever `start` function the
//! module declares, and that function is guest code of the component
//! instance the core instance belongs to. It can call a built-in, so
//! it needs what every other piece of guest code has: a task on the
//! store's stack of current scopes, with one thread of its own for
//! the context slots and for a wait to park on.
//!
//! The call is implicitly synchronous — the reference gives a `start`
//! function no way to return a status word — so the instance may not
//! suspend while it runs. A `waitable-set.wait` from a start function
//! therefore gives way to the ready work of its own instance and then
//! fails with the cannot-block cause, which is the trap the corpus
//! expects.
//!
//! [`StartTask`] is that task, held for the length of the
//! instantiation. Its drop ends the task whether the instantiation
//! returned or trapped, which puts the may-not-suspend flag back.

use std::sync::{Arc, Mutex};

use crate::concurrency::{InstanceId, TaskId};
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
use crate::resource::HandleTables;

/// The task one core module's instantiation runs in, in flight.
///
/// The value is a guard. Build it immediately before the core
/// instantiation and drop it immediately after, whether the
/// instantiation returned or failed.
pub struct StartTask {
    /// The store's records, through which the drop ends the task.
    /// `None` for a module that belongs to no component instance,
    /// which is an adapter module: it has no instance to run in and
    /// no built-in to call.
    tables: Option<Arc<Mutex<HandleTables>>>,
    /// The task the instantiation runs as.
    task: Option<TaskId>,
}

impl StartTask {
    /// Enter the instantiation of a core module of `instance`: a
    /// task with one fresh thread becomes the current scope, and the
    /// instance may not suspend until the instantiation ends.
    pub fn enter(tables: &Arc<Mutex<HandleTables>>, instance: Option<InstanceId>) -> Result<Self> {
        let Some(instance) = instance else {
            return Ok(Self {
                tables: None,
                task: None,
            });
        };
        let mut guard = tables
            .lock()
            .map_err(|_| Error::internal("resource handle tables lock poisoned"))?;
        let task = guard.tasks.push_task(None, None, instance)?;
        guard.tasks.start_task(task);
        guard.tasks.hold_may_not_suspend(task).ok_or_else(|| {
            Error::internal("a core instantiation named an instance the store does not hold")
        })?;
        drop(guard);
        Ok(Self {
            tables: Some(tables.clone()),
            task: Some(task),
        })
    }
}

impl Drop for StartTask {
    /// End the instantiation's task: the scope is popped, the record
    /// and its thread leave the store, and the may-not-suspend flag
    /// goes back to the value the entry saved. A scope a trapping
    /// start function left above the task goes with it. A poisoned
    /// lock leaves all of it undone, because there is no record left
    /// to read or write.
    fn drop(&mut self) {
        let (Some(tables), Some(task)) = (self.tables.take(), self.task.take()) else {
            return;
        };
        let Ok(mut guard) = tables.lock() else {
            return;
        };
        // The task carries no borrow of its own, so there is nothing
        // for an exit to check.
        guard.abandon_task(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store's records with one component instance in them.
    fn records() -> (Arc<Mutex<HandleTables>>, InstanceId) {
        let mut handles = HandleTables::new();
        let instance = handles.tasks.insert_instance();
        (Arc::new(Mutex::new(handles)), instance)
    }

    /// Whether the instance may suspend, and how many tasks,
    /// threads, and scopes the store holds.
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
                .may_not_suspend,
            guard.tasks.task_count(),
            guard.tasks.thread_count(),
            guard.tasks.scopes().len(),
        )
    }

    #[wcmp_macros::test]
    fn it_runs_a_start_function_in_a_task_that_may_not_suspend() {
        let (tables, instance) = records();
        assert_eq!(state(&tables, instance), (false, 0, 0, 0));

        let start = StartTask::enter(&tables, Some(instance)).expect("enter the instantiation");
        assert_eq!(
            state(&tables, instance),
            (true, 1, 1, 1),
            "one task with one thread is the current scope, and the instance may not suspend"
        );

        drop(start);
        assert_eq!(
            state(&tables, instance),
            (false, 0, 0, 0),
            "the task, its thread, and the flag all end with the instantiation"
        );
    }

    #[wcmp_macros::test]
    fn it_ends_the_task_when_the_start_function_traps() {
        let (tables, instance) = records();
        let failed: std::result::Result<(), ()> = {
            let _start = StartTask::enter(&tables, Some(instance)).expect("enter");
            Err(())
        };
        assert!(failed.is_err());
        assert_eq!(
            state(&tables, instance),
            (false, 0, 0, 0),
            "nothing of the failed instantiation is left in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_pushes_no_task_for_a_module_that_belongs_to_no_component_instance() {
        let (tables, instance) = records();
        let start = StartTask::enter(&tables, None).expect("enter");
        assert_eq!(
            state(&tables, instance),
            (false, 0, 0, 0),
            "an adapter module runs in no instance, so it takes no task"
        );
        drop(start);
    }
}
