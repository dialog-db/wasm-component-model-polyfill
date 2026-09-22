//! Baseline tests for the task a destructor runs as.
//!
//! The reference lifts a resource's destructor as a synchronous
//! function of one `u32` parameter and lowers a call to it, so every
//! destructor run is a task with one fresh thread: the task is the
//! current scope while the destructor runs and is gone after it, and
//! the thread's context slots start at zero and end with it. A
//! destructor therefore sees zeros, what it sets does not reach the
//! thread that dropped the handle, and a destructor that drops
//! another resource nests a second task the same way.
//!
//! The rule holds whoever released the handle: a guest's
//! `resource.drop` and the host's own release run the destructor the
//! same way. The tests here drop from a guest; the host's release of
//! a resource it implements itself is a unit test beside
//! `StoreContext::resource_drop`, because a host destructor closure
//! reads the store's records rather than a context slot.
//!
//! The instance may still be left while a destructor runs, which the
//! reference states by clearing the may-leave flag around a realloc
//! and a post-return and not around a destructor. Every destructor
//! below proves it by calling a host import: a call that leaves the
//! instance traps while the flag is clear, so a destructor that
//! reaches the host at all is a destructor whose instance may be
//! left.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use crate::store::StoreInternalExt;
use crate::{Component, Engine, HostCall, Linker, Result, Store, Val};
use wcmp_macros::component;

/// What a host function saw of the store's records while it ran.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Seen {
    /// How deep the stack of current scopes was.
    scopes: usize,
    /// How many task records the store held.
    tasks: usize,
    /// How many subtask records the store held.
    subtasks: usize,
    /// How many thread records the store held.
    threads: usize,
    /// The context slots of the current thread.
    context: [i32; 2],
}

/// A component whose resource has a destructor that reads its
/// context slot, writes it, and calls the host from inside the
/// destructor. Its export sets a slot of its own, drops an owned
/// handle, and returns the slot it reads back afterwards.
const GUEST_DROP: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $get (canon context.get i32 0))
      (core func $set (canon context.set i32 0))
      (core module $Dtor
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "dtor") (param i32)
          (if (i32.ne (call $get) (i32.const 0)) (then unreachable))
          (call $set (i32.const 0xdead))
          (drop (call $probe (i32.const 1)))))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "probe" (func $probe'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $M
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "run") (result i32)
          (call $set (i32.const 0x1234))
          (call $drop (call $new (i32.const 100)))
          (call $get)))
      (core instance $m (instantiate $M (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

/// The same component, with a destructor that traps once the host
/// has read the records.
const GUEST_DROP_TRAPS: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $set (canon context.set i32 0))
      (core module $Dtor
        (import "" "set" (func $set (param i32)))
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "dtor") (param i32)
          (call $set (i32.const 0xdead))
          (drop (call $probe (i32.const 1)))
          (unreachable)))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "set" (func $set))
        (export "probe" (func $probe'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "run") (result i32)
          (call $drop (call $new (i32.const 100)))
          (i32.const 0)))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

/// A component whose destructor drops a resource of its own, so a
/// second destructor task nests inside the first. The inner
/// destructor calls the host. The export returns what the outer
/// destructor read back after the nested drop, which is its own slot
/// and not the inner destructor's.
const NESTED_GUEST_DROP: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $get (canon context.get i32 0))
      (core func $set (canon context.set i32 0))
      (core module $Inner
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "dtor") (param i32)
          (if (i32.ne (call $get) (i32.const 0)) (then unreachable))
          (call $set (i32.const 0x3333))
          (drop (call $probe (i32.const 1)))))
      (core instance $inner (instantiate $Inner (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "probe" (func $probe'))))))
      (type $r2 (resource (rep i32) (dtor (core func $inner "dtor"))))
      (core func $new2 (canon resource.new $r2))
      (core func $drop2 (canon resource.drop $r2))
      (core module $Outer
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "new2" (func $new2 (param i32) (result i32)))
        (import "" "drop2" (func $drop2 (param i32)))
        (global $seen (mut i32) (i32.const 0))
        (func (export "dtor") (param i32)
          (if (i32.ne (call $get) (i32.const 0)) (then unreachable))
          (call $set (i32.const 0x2222))
          (call $drop2 (call $new2 (i32.const 200)))
          (global.set $seen (call $get)))
        (func (export "seen") (result i32) (global.get $seen)))
      (core instance $outer (instantiate $Outer (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "new2" (func $new2))
        (export "drop2" (func $drop2))))))
      (type $r1 (resource (rep i32) (dtor (core func $outer "dtor"))))
      (core func $new1 (canon resource.new $r1))
      (core func $drop1 (canon resource.drop $r1))
      (core module $M
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "new1" (func $new1 (param i32) (result i32)))
        (import "" "drop1" (func $drop1 (param i32)))
        (import "" "seen" (func $seen (result i32)))
        (func (export "run") (result i32)
          (call $set (i32.const 0x1111))
          (call $drop1 (call $new1 (i32.const 100)))
          (if (i32.ne (call $get) (i32.const 0x1111)) (then unreachable))
          (call $seen)))
      (core instance $m (instantiate $M (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "new1" (func $new1))
        (export "drop1" (func $drop1))
        (export "seen" (func $outer "seen"))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

/// A component that hands the host an owned handle and records what
/// its destructors saw. The destructor of the handle the host holds
/// drops a resource of its own, so the host's release nests two
/// destructor tasks.
const HOST_RELEASE: &[u8] = component!(
    r#"
    (component
      (core func $get (canon context.get i32 0))
      (core func $set (canon context.set i32 0))
      (core module $Inner
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (global $saw (mut i32) (i32.const -1))
        (func (export "dtor") (param i32)
          (global.set $saw (call $get))
          (call $set (i32.const 0x3333)))
        (func (export "saw") (result i32) (global.get $saw)))
      (core instance $inner (instantiate $Inner (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))))))
      (type $r2 (resource (rep i32) (dtor (core func $inner "dtor"))))
      (core func $new2 (canon resource.new $r2))
      (core func $drop2 (canon resource.drop $r2))
      (core module $Outer
        (import "" "get" (func $get (result i32)))
        (import "" "set" (func $set (param i32)))
        (import "" "new2" (func $new2 (param i32) (result i32)))
        (import "" "drop2" (func $drop2 (param i32)))
        (global $saw (mut i32) (i32.const -1))
        (global $kept (mut i32) (i32.const -1))
        (func (export "dtor") (param i32)
          (global.set $saw (call $get))
          (call $set (i32.const 0x2222))
          (call $drop2 (call $new2 (i32.const 200)))
          (global.set $kept (call $get)))
        (func (export "saw") (result i32) (global.get $saw))
        (func (export "kept") (result i32) (global.get $kept)))
      (core instance $outer (instantiate $Outer (with "" (instance
        (export "get" (func $get))
        (export "set" (func $set))
        (export "new2" (func $new2))
        (export "drop2" (func $drop2))))))
      (type $r1 (resource (rep i32) (dtor (core func $outer "dtor"))))
      (core func $new1 (canon resource.new $r1))
      (core module $M
        (import "" "new1" (func $new1 (param i32) (result i32)))
        (import "" "set" (func $set (param i32)))
        (func (export "make") (result i32)
          (call $set (i32.const 0x1111))
          (call $new1 (i32.const 100))))
      (core instance $m (instantiate $M (with "" (instance
        (export "new1" (func $new1))
        (export "set" (func $set))))))
      (export $t "t" (type $r1))
      (func (export "make") (result (own $t))
        (canon lift (core func $m "make")))
      (func (export "outer-saw") (result s32)
        (canon lift (core func $outer "saw")))
      (func (export "outer-kept") (result s32)
        (canon lift (core func $outer "kept")))
      (func (export "inner-saw") (result s32)
        (canon lift (core func $inner "saw"))))
    "#
);

/// Read the store's records as they stand now, with `context` taken
/// from the current thread when there is one. A macro rather than a
/// function because the tables are a workspace-internal type that a
/// test outside the crate cannot name.
macro_rules! seen {
    ($tables:expr) => {{
        let guard = $tables.lock().expect("handle tables");
        Seen {
            scopes: guard.tasks.scopes().len(),
            tasks: guard.tasks.task_count(),
            subtasks: guard.tasks.subtask_count(),
            threads: guard.tasks.thread_count(),
            context: guard
                .tasks
                .current_thread()
                .and_then(|thread| guard.tasks.thread(thread))
                .map(|record| record.context)
                .unwrap_or([0; 2]),
        }
    }};
}

/// Instantiate `bytes` with a host `probe` function that records the
/// store's records while it runs, and call the `run` export.
/// Answers what the call produced, what the host saw during it, and
/// what the store held after it.
async fn run_with_probe(bytes: &[u8]) -> (Result<Box<[Val]>>, Seen, Seen) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let tables = store.internal().tables_handle();
    let during: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Seen::default()));
    let recorded = during.clone();

    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "probe",
            move |_: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                *recorded.lock().expect("record") = seen!(&tables);
                Ok(x)
            },
        )
        .expect("the registration");

    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let result = run.call(&mut store, &[]).await;

    let mut after = seen!(store.internal().tables());
    after.context = [0; 2];
    let during = during.lock().expect("record").clone();
    (result, during, after)
}

#[wcmp_macros::test]
async fn it_runs_a_guest_drop_destructor_on_a_task_of_its_own() {
    let (result, during, after) = run_with_probe(GUEST_DROP).await;

    assert_eq!(
        result.expect("the call returned").first().cloned(),
        Some(Val::U32(0x1234)),
        "the destructor found zero in slot 0 and set it on its own thread, and \
         the task that dropped the handle still reads its own 0x1234"
    );
    assert_eq!(
        during,
        Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            context: [0xdead, 0],
        },
        "the destructor's task sits on the export's task, with the subtask of \
         the host call on top of both, which the host call could only reach \
         because the instance may still be left; its own fresh thread carries \
         the slot it set"
    );
    assert_eq!(
        after,
        Seen::default(),
        "the destructor's task and its thread are gone"
    );
}

#[wcmp_macros::test]
async fn it_ends_the_destructor_task_when_the_destructor_traps() {
    let (result, during, after) = run_with_probe(GUEST_DROP_TRAPS).await;

    assert!(
        result.is_err(),
        "the trapping destructor failed the call that dropped the handle"
    );
    assert_eq!(
        during,
        Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            context: [0xdead, 0],
        },
        "the destructor's task was the current scope while it ran"
    );
    assert_eq!(
        after,
        Seen::default(),
        "nothing of the failed destructor is left in the store"
    );
}

#[wcmp_macros::test]
async fn it_nests_a_task_for_a_destructor_that_drops_another_resource() {
    let (result, during, after) = run_with_probe(NESTED_GUEST_DROP).await;

    assert_eq!(
        result.expect("the call returned").first().cloned(),
        Some(Val::U32(0x2222)),
        "the outer destructor read its own slot back after the nested drop"
    );
    assert_eq!(
        during,
        Seen {
            scopes: 4,
            tasks: 3,
            subtasks: 1,
            threads: 3,
            context: [0x3333, 0],
        },
        "the inner destructor's task nests inside the outer one, which nests \
         inside the export's, and the inner thread's slots are its own"
    );
    assert_eq!(
        after,
        Seen::default(),
        "both destructor tasks and both threads are gone"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_host_release_destructor_on_a_task_of_its_own() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, HOST_RELEASE)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let make = instance.get_func("make").expect("make export");
    let made = make.call(&mut store, &[]).await.expect("make returned");
    let Some(Val::Own(handle)) = made.first().cloned() else {
        panic!("make returns an owned handle, got {made:?}");
    };

    store.resource_drop(handle).expect("the host releases it");

    let mut after = seen!(store.internal().tables());
    after.context = [0; 2];
    assert_eq!(
        after,
        Seen::default(),
        "both destructor tasks and their threads ended with the release"
    );

    let outer_saw = instance.get_func("outer-saw").expect("outer-saw export");
    let outer_saw = outer_saw.call(&mut store, &[]).await.expect("call");
    assert_eq!(
        outer_saw.first().cloned(),
        Some(Val::S32(0)),
        "the destructor of the handle the host released started with zero slots, \
         though the task that made the handle had set one"
    );

    let inner_saw = instance.get_func("inner-saw").expect("inner-saw export");
    let inner_saw = inner_saw.call(&mut store, &[]).await.expect("call");
    assert_eq!(
        inner_saw.first().cloned(),
        Some(Val::S32(0)),
        "so did the destructor it nested by dropping a resource of its own"
    );

    let outer_kept = instance.get_func("outer-kept").expect("outer-kept export");
    let outer_kept = outer_kept.call(&mut store, &[]).await.expect("call");
    assert_eq!(
        outer_kept.first().cloned(),
        Some(Val::S32(0x2222)),
        "and the outer destructor read its own slot back after the nested drop"
    );
}
