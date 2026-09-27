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
//! runs. It does not clear it around a destructor. The flag is the
//! core global the instance's adapters compile against, so clearing
//! it here is what a `resource.new` in a realloc, a host import in a
//! post-return, and an adapter's own guest-to-guest call all read.
//!
//! [`BoundaryCall`] is that entry, held for the length of the call.
//! The task half is a guard: its drop pops the task, so a call that
//! failed or trapped leaves nothing on the stack. The flag half is
//! not, because writing a core global needs the store, which a drop
//! cannot reach: [`BoundaryCall::end`] takes the store and gives the
//! flag back, and every call site ends the call that way whether the
//! call returned or failed.

use std::sync::{Arc, Mutex};

use wasm_runtime_layer::AsContextMut;

use crate::abi::instance::BoundaryInstance;
use crate::abi::instance_flags::InstanceFlags;
use crate::concurrency::{InstanceId, TaskId};
use crate::error::{Error, Result};
use crate::internal::ErrorInternal;
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
    /// The may-leave flag the entry cleared, with the value
    /// [`BoundaryCall::end`] gives back.
    may_leave: Option<(InstanceFlags, bool)>,
    /// The task the call runs as, when it runs as one.
    task: Option<TaskId>,
}

impl BoundaryCall {
    /// Enter a call into the `cabi_realloc` of `instance`: a task
    /// with one fresh thread becomes the current scope, and the
    /// instance may not be left until the call ends.
    pub fn realloc(instance: &BoundaryInstance, store: impl AsContextMut) -> Result<Self> {
        Self::enter(instance, store, true)
    }

    /// Enter a call into the `post-return` of an export of
    /// `instance`: the instance may not be left until the call ends.
    /// The reference calls the `post-return` from the export's own
    /// task, so the call pushes no task of its own.
    pub fn post_return(instance: &BoundaryInstance, store: impl AsContextMut) -> Result<Self> {
        Self::enter(instance, store, false)
    }

    /// Enter a call into the destructor of a resource: a task with
    /// one fresh thread becomes the current scope. `instance` is the
    /// component instance that implements the resource, and `None`
    /// when the host does. The instance may still be left, because
    /// the reference clears the flag around a realloc and a
    /// post-return and not around a destructor.
    ///
    /// The instance may not suspend until the call ends. The
    /// reference says a destructor may not block, and Wasmtime runs
    /// one as a synchronous call whose block traps with
    /// `Trap::CannotBlockSyncTask`. The may-not-suspend flag is what
    /// the suspend seam reads for that rule, so a block anywhere
    /// inside the destructor fails with the cannot-block cause,
    /// whatever provider the engine selected. A host destructor has no
    /// instance to mark and needs none: it is a synchronous closure,
    /// which cannot reach the seam.
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
            Some(instance) => guard.tasks.push_task(None, None, instance)?,
            None => guard.tasks.push_task_without_instance()?,
        };
        guard.tasks.start_task(task);
        let held = instance.is_none() || guard.tasks.hold_may_not_suspend(task).is_some();
        drop(guard);
        // The guard owns the task from here on, so a refusal still
        // takes the task off the stack when the guard drops.
        call.task = Some(task);
        call.tables = Some(tables.clone());
        if !held {
            return Err(Error::internal(
                "a resource destructor named an instance the store does not hold",
            ));
        }
        Ok(call)
    }

    /// Enter the call, pushing a task of its own when `as_task`.
    ///
    /// An instance with no tables reaches no records, so the call
    /// enters nothing and the guard has nothing to undo. That holds
    /// whether or not the instance names an identity: the task and
    /// the flag both live in the records, and the records are
    /// reached through the tables alone, so
    /// `BoundaryInstance::without_tables(Some(id))` is as much of a
    /// no-op guard as the `None` form.
    ///
    /// An instance that has tables but no identity is not a crossing
    /// any caller builds: entering it would run the call with no
    /// task, no fresh thread, and the may-leave flag of nobody, so
    /// it is an internal error instead. An instance the
    /// instantiation minted no may-leave flag for is refused the
    /// same way and for the same reason: the flag to clear is
    /// nobody's.
    ///
    /// Every refusal therefore comes before the flag is cleared,
    /// including the lock the task is pushed under, so that a
    /// refused entry leaves nothing for [`Self::end`] to give back.
    fn enter(
        instance: &BoundaryInstance,
        mut store: impl AsContextMut,
        as_task: bool,
    ) -> Result<Self> {
        let mut call = Self {
            tables: None,
            may_leave: None,
            task: None,
        };
        let Some(tables) = instance.tables() else {
            return Ok(call);
        };
        let Some(id) = instance.id() else {
            return Err(Error::internal(
                "a call into a guest names handle tables but no component instance",
            ));
        };
        let Some(flags) = instance.flags() else {
            return Err(Error::internal(
                "a call into a guest names a component instance with no may-leave flag",
            ));
        };
        let mut records = if as_task {
            Some(
                tables
                    .lock()
                    .map_err(|_| Error::internal("resource handle tables lock poisoned"))?,
            )
        } else {
            None
        };
        let old = flags.set_may_leave(store.as_context_mut(), false)?;
        call.may_leave = Some((flags.clone(), old));
        if let Some(guard) = records.as_mut() {
            let task = guard.tasks.push_task(None, None, id)?;
            guard.tasks.start_task(task);
            call.task = Some(task);
        }
        drop(records);
        call.tables = Some(tables.clone());
        Ok(call)
    }

    /// End the call: the instance may be left again, and the task
    /// and its thread leave the store.
    ///
    /// Every call site ends the call this way, whether the call
    /// returned or failed, because the flag lives in a core global
    /// that only the store can write and a drop reaches no store.
    pub fn end(mut self, store: impl AsContextMut) -> Result<()> {
        let restored = match self.may_leave.take() {
            Some((flags, old)) => flags.set_may_leave(store, old).map(|_| ()),
            None => Ok(()),
        };
        drop(self);
        restored
    }
}

impl Drop for BoundaryCall {
    /// Take the call's task and its thread out of the store. A
    /// poisoned lock leaves them, because there is no record left to
    /// read or write.
    ///
    /// The flag is not given back here: writing the core global it
    /// lives in needs the store, which a drop cannot reach, so
    /// [`BoundaryCall::end`] is what gives it back. A guard that
    /// still holds a cleared flag when it drops has been dropped by
    /// hand rather than ended, which is a mistake of this crate's
    /// own.
    fn drop(&mut self) {
        // A guard dropped while a panic unwinds past it is not that
        // mistake, and asserting there would turn the panic into an
        // abort, so the check stands down for it.
        debug_assert!(
            self.may_leave.is_none() || std::thread::panicking(),
            "a call into a guest was dropped rather than ended, leaving its \
             instance unleavable"
        );
        let Some(tables) = self.tables.take() else {
            return;
        };
        let Ok(mut guard) = tables.lock() else {
            return;
        };
        if let Some(task) = self.task.take() {
            // The scope is popped, the lends recorded against the
            // task are given back, the record and its thread are
            // removed, and a scope a trapping call left above it
            // goes with them. The one step an exit would add is
            // skipped: the count of borrows still owed to the task
            // is read and discarded rather than turned into a trap.
            //
            // Only a realloc and a destructor reach here: a
            // post-return enters with no task of its own, so it
            // never takes this branch. Neither of the two is owed a
            // borrow by its own parameters, which is the
            // reference's route to the count — a `canon lower` of a
            // `borrow<T>` the task is lifted with. A realloc is
            // lifted with four `i32` and a destructor with one
            // `u32`, so neither signature carries one.
            //
            // The other route is a `canon lower` that runs inside
            // the task, which is an import the task called; this
            // crate counts a lowering made while the task's own
            // host call runs against the task as well. The
            // reference traps on such a lower while the instance
            // may not be left, which is the whole of a realloc, so
            // a realloc cannot reach the point where it owes one.
            // The polyfill's own lowering reads the same flag, so
            // that rule holds here for the same reason it holds in
            // the reference.
            //
            // A destructor runs with the flag set and is arbitrary
            // guest code: it can call an import and receive a borrow
            // lowered into its own task, and the reference traps
            // when such a task returns holding one. No directive of
            // the corpus reaches that, and checking here would need
            // a cause and a test of its own, so the count stays
            // unchecked.
            guard.abandon_task(task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::runtime_state::AbiRuntimeState;
    use crate::concurrency::Scope;
    use crate::engine::Engine;
    use crate::executor::ir::{CanonOptions, DataModel, StringEncoding};
    use crate::resource::TableId;
    use crate::store::Store;
    use crate::store::{StoreContextInternalExt, StoreInternalExt};

    /// The declared options of a crossing that names the first
    /// component instance of its instantiation and nothing else.
    fn declared() -> Arc<CanonOptions> {
        Arc::new(CanonOptions {
            instance: 0,
            memory: None,
            realloc: None,
            post_return: None,
            async_: false,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        })
    }

    /// A store with one component instance in its records, the
    /// instance's identity, and the crossing that instance's options
    /// resolve to. The instance's may-leave flag is a real core
    /// global of that store, as an instantiation mints it, so what a
    /// call writes here is what a built-in of the same store reads.
    fn records() -> (Store<()>, InstanceId, BoundaryInstance) {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = store
            .internal()
            .lock_tables()
            .expect("handle tables")
            .tasks
            .insert_instance();
        let flags = InstanceFlags::new(store.internal().context().internal().runtime_mut());
        let tables = store.internal().tables_handle();
        let state = Arc::new(Mutex::new(
            AbiRuntimeState::with_slabs(
                0,
                0,
                0,
                0,
                Vec::new(),
                vec![instance],
                vec![TableId::fresh()],
            )
            .with_instance_flags(vec![flags]),
        ));
        let (_, boundary) =
            BoundaryInstance::resolve(&declared(), &state, &tables).expect("resolve the crossing");
        (store, instance, boundary)
    }

    /// A store's records and a crossing that reaches them but no
    /// component instance, because the instantiation filled no
    /// instance slot for its options to name.
    fn records_without_an_instance() -> (Arc<Mutex<HandleTables>>, BoundaryInstance) {
        let tables = Arc::new(Mutex::new(HandleTables::new()));
        let state = Arc::new(Mutex::new(AbiRuntimeState::with_slabs(
            0,
            0,
            0,
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )));
        let (_, boundary) =
            BoundaryInstance::resolve(&declared(), &state, &tables).expect("resolve the crossing");
        (tables, boundary)
    }

    /// Whether the instance may be left, read off the core global
    /// the crossing carries, and how many tasks, threads, and scopes
    /// the store holds.
    fn state(store: &mut Store<()>, boundary: &BoundaryInstance) -> (bool, usize, usize, usize) {
        let may_leave = boundary
            .flags()
            .expect("the crossing carries the instance's flag")
            .may_leave(store.internal().context().internal().runtime_mut())
            .expect("read the flag");
        let tables = boundary.tables().expect("the crossing reaches the records");
        let guard = tables.lock().expect("handle tables");
        (
            may_leave,
            guard.tasks.task_count(),
            guard.tasks.thread_count(),
            guard.tasks.scopes().len(),
        )
    }

    #[wcmp_macros::test]
    fn it_runs_a_realloc_as_a_task_with_one_thread_that_may_not_leave() {
        let (mut store, _, boundary) = records();
        assert_eq!(state(&mut store, &boundary), (true, 0, 0, 0));

        let call = BoundaryCall::realloc(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        )
        .expect("enter the realloc");
        let (may_leave, tasks, threads, scopes) = state(&mut store, &boundary);
        assert!(
            !may_leave,
            "the instance may not be left while a realloc runs"
        );
        assert_eq!((tasks, threads, scopes), (1, 1, 1), "one task, one thread");
        {
            let tables = boundary.tables().expect("records");
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

        call.end(store.internal().context().internal().runtime_mut())
            .expect("end the realloc");
        assert_eq!(
            state(&mut store, &boundary),
            (true, 0, 0, 0),
            "the task, its thread, and the cleared flag all end with the call"
        );
    }

    #[wcmp_macros::test]
    fn it_makes_the_cleared_flag_visible_through_the_instances_flags_global() {
        // The built-ins read the instance's may-leave flag off the
        // core global its adapters compile against, which is the
        // global this crossing carries. A call the polyfill makes
        // into the guest therefore shows through that global for as
        // long as it runs, and shows the old value again once it has
        // ended.
        let (mut store, _, boundary) = records();
        let flags = boundary.flags().expect("the instance's flag").clone();
        assert!(
            flags
                .may_leave(store.internal().context().internal().runtime_mut())
                .expect("read the flag"),
            "a fresh instance may be left"
        );

        let call = BoundaryCall::post_return(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        )
        .expect("enter the post-return");
        assert!(
            !flags
                .may_leave(store.internal().context().internal().runtime_mut())
                .expect("read the flag"),
            "the global a built-in reads is clear while the post-return runs"
        );

        call.end(store.internal().context().internal().runtime_mut())
            .expect("end the post-return");
        assert!(
            flags
                .may_leave(store.internal().context().internal().runtime_mut())
                .expect("read the flag"),
            "the global holds the value the call was owed once it has ended"
        );
    }

    #[wcmp_macros::test]
    fn it_runs_a_post_return_inside_the_task_that_called_the_export() {
        let (mut store, instance, boundary) = records();
        let export = store
            .internal()
            .lock_tables()
            .expect("handle tables")
            .tasks
            .push_task(None, None, instance)
            .expect("room under the record cap");

        let call = BoundaryCall::post_return(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        )
        .expect("enter the post-return");
        let (may_leave, tasks, threads, scopes) = state(&mut store, &boundary);
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
            store
                .internal()
                .lock_tables()
                .expect("handle tables")
                .tasks
                .current_scope(),
            Some(Scope::Task(export)),
            "the export's own task is still the current scope"
        );

        call.end(store.internal().context().internal().runtime_mut())
            .expect("end the post-return");
        assert_eq!(
            state(&mut store, &boundary),
            (true, 1, 1, 1),
            "the flag is given back and the export's task is untouched"
        );
    }

    #[wcmp_macros::test]
    fn it_ends_the_call_when_the_call_fails() {
        // A realloc that traps leaves the call site with a failure
        // rather than a result, and the call site ends the call all
        // the same.
        let (mut store, _, boundary) = records();
        let call = BoundaryCall::realloc(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        )
        .expect("enter the realloc");
        let failed: std::result::Result<(), ()> = Err(());
        call.end(store.internal().context().internal().runtime_mut())
            .expect("end the realloc");
        assert!(failed.is_err());
        assert_eq!(
            state(&mut store, &boundary),
            (true, 0, 0, 0),
            "nothing of the failed call is left in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_does_nothing_for_a_crossing_that_reaches_no_records() {
        // A copy between two guest memories names no tables, so
        // there is nothing to push a task into and no flag to clear.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let call = BoundaryCall::realloc(
            &BoundaryInstance::without_tables(None),
            store.internal().context().internal().runtime_mut(),
        )
        .expect("enter the realloc");
        call.end(store.internal().context().internal().runtime_mut())
            .expect("end the realloc");
    }

    #[wcmp_macros::test]
    fn it_refuses_a_realloc_whose_crossing_names_no_component_instance() {
        // Records but no instance is a combination no caller builds.
        // Entering it would run the realloc with no task, no fresh
        // thread, and nobody's flag cleared, so the entry fails
        // rather than handing back a guard that does nothing.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (tables, boundary) = records_without_an_instance();
        let Err(error) = BoundaryCall::realloc(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        ) else {
            panic!("the realloc is refused");
        };
        assert!(
            matches!(error, Error::Internal { .. }),
            "the refusal is a structured internal error, not a trap: {error:?}"
        );
        let guard = tables.lock().expect("handle tables");
        assert_eq!(
            (
                guard.tasks.task_count(),
                guard.tasks.thread_count(),
                guard.tasks.scopes().len()
            ),
            (0, 0, 0),
            "the refused entry left nothing in the store"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_post_return_whose_crossing_names_no_component_instance() {
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let (_, boundary) = records_without_an_instance();
        let Err(error) = BoundaryCall::post_return(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        ) else {
            panic!("the post-return is refused");
        };
        assert!(
            matches!(error, Error::Internal { .. }),
            "the refusal is a structured internal error, not a trap: {error:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_refuses_a_realloc_whose_instance_has_no_may_leave_flag() {
        // An instantiation that minted no flag for the instance
        // leaves the entry with nobody's flag to clear, so it fails
        // rather than running the realloc with the flag of no
        // instance at all. The flag is cleared before the task is
        // pushed, so the refusal leaves no task behind either.
        let engine = Engine::new().expect("engine");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = store
            .internal()
            .lock_tables()
            .expect("handle tables")
            .tasks
            .insert_instance();
        let tables = store.internal().tables_handle();
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
            BoundaryInstance::resolve(&declared(), &state, &tables).expect("resolve the crossing");

        let Err(error) = BoundaryCall::realloc(
            &boundary,
            store.internal().context().internal().runtime_mut(),
        ) else {
            panic!("the realloc is refused");
        };
        assert!(
            matches!(error, Error::Internal { .. }),
            "the refusal is a structured internal error, not a trap: {error:?}"
        );
        let guard = tables.lock().expect("handle tables");
        assert_eq!(
            (
                guard.tasks.task_count(),
                guard.tasks.thread_count(),
                guard.tasks.scopes().len()
            ),
            (0, 0, 0),
            "the refused entry left nothing in the store"
        );
    }
}
