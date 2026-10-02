// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//!
//! A destructor may not block, as a synchronous call may not. The
//! reference says so of every destructor, and Wasmtime runs one as a
//! synchronous call whose block traps with `CannotBlockSyncTask`. A
//! block inside a destructor therefore fails with the cannot-block
//! cause, whatever provider the engine selected, and an
//! import lowered with the `async` option, which does not block,
//! leaves its host task to a later turn.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex};

use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    Accessor, Component, Engine, Error, HostCall, Instance, Linker, ResourceHandle, Result,
    SchedulerCause, Store, Val,
};
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
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
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

/// A component whose locally-defined resource has a destructor that
/// calls two host imports. The first is a synchronous `probe`, which
/// reads whether a turn of the store is running. The second is an
/// `async`-typed `answer` lowered without the `async` option, so the
/// destructor expects its result when the call returns; it keeps the
/// result in a global the host reads back through an export.
const HOST_RELEASE_CALLS_A_HOST_ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $answer' (canon lower (func $answer)))
      (core module $Dtor
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "answer" (func $answer (param i32) (result i32)))
        (global $answered (mut i32) (i32.const -1))
        (func (export "dtor") (param i32)
          (drop (call $probe (i32.const 1)))
          (global.set $answered (call $answer (i32.const 21))))
        (func (export "answered") (result i32) (global.get $answered)))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "probe" (func $probe'))
        (export "answer" (func $answer'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (func (export "make") (result i32)
          (call $new (i32.const 100))))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))))))
      (export $t "t" (type $r))
      (func (export "make") (result (own $t))
        (canon lift (core func $m "make")))
      (func (export "answered") (result s32)
        (canon lift (core func $dtor "answered"))))
    "#
);

/// Every message in an error's source chain, joined so a cause the
/// substrate wrapped can be matched wherever it put it. A failure
/// raised inside a host function reaches the call that ran the
/// destructor through the runtime's trap surface, which keeps the
/// structured cause as a source rather than as the top-level error.
fn chain(error: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&link.to_string());
        current = link.source();
    }
    out
}

/// What the destructor's `probe` call recorded, and the release it
/// was run for.
struct Released {
    store: Store<()>,
    instance: Instance,
    handle: ResourceHandle,
    /// Whether a turn of the store was running while the destructor
    /// ran.
    in_turn: Arc<Mutex<Option<bool>>>,
    /// How a driver the destructor's host call entered came out.
    driver: Arc<Mutex<Option<String>>>,
}

/// Instantiate `bytes`, whose destructor calls a synchronous `probe`
/// import, with `register` registering the rest of its imports, and
/// take an owned handle out of its `make` export.
///
/// The `probe` records whether a turn of the store was running while
/// the destructor ran, and enters a driver of the same store and
/// records how it came out. The driver is refused before it creates
/// or queues anything, so the destructor goes on as it would have.
async fn host_release_caller<F>(bytes: &[u8], register: F) -> Released
where
    F: FnOnce(&mut Linker<()>),
{
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let in_turn: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let driver: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let turn_recorded = in_turn.clone();
    let driver_recorded = driver.clone();
    linker
        .root()
        .func_wrap(
            "probe",
            move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
                let store = call.store();
                *turn_recorded.lock().expect("record") = Some(store.internal().turn_in_flight());
                let mut reborrowed = store.internal().reborrow();
                let mut nested = Box::pin(reborrowed.internal().run_concurrent(async |_| ()));
                let outcome = nested
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()));
                *driver_recorded.lock().expect("record") = Some(match outcome {
                    Poll::Ready(Err(error)) => error.to_string(),
                    Poll::Ready(Ok(())) => "the driver succeeded".to_owned(),
                    Poll::Pending => "the driver returned pending".to_owned(),
                });
                Ok(x)
            },
        )
        .expect("the registration");
    register(&mut linker);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let make = instance.get_func("make").expect("make export");
    let made = make.call(&mut store, &[]).await.expect("make returned");
    let Some(Val::Own(handle)) = made.first().cloned() else {
        panic!("make returns an owned handle, got {made:?}");
    };
    Released {
        store,
        instance,
        handle,
        in_turn,
        driver,
    }
}

/// Register `answer` with a typed concurrent entry whose future never
/// resolves.
fn never_answers(linker: &mut Linker<()>) {
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (_x,): (u32,)| {
            core::future::pending::<Result<u32>>()
        })
        .expect("the registration");
}

/// A future that is pending the first time it is polled and ready
/// with `value` afterwards. It wakes the waker it was polled with
/// before it parks, as a future waiting on a timer has its host do.
struct PendingOnce<V> {
    polled: bool,
    value: Option<V>,
}

impl<V> PendingOnce<V> {
    fn new(value: V) -> Self {
        Self {
            polled: false,
            value: Some(value),
        }
    }
}

impl<V: Unpin> Future for PendingOnce<V> {
    type Output = V;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<V> {
        let this = self.get_mut();
        if this.polled {
            return Poll::Ready(this.value.take().expect("the future is polled once ready"));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Check what a release whose destructor blocked left behind: the
/// cannot-block cause with Wasmtime's wording, a destructor that ran
/// inside a turn that has ended, and nothing of the destructor, its
/// import's call, or a host task in the store.
fn assert_blocked_release(released: &mut Released, err: &Error) {
    assert!(
        chain(err).contains(&Error::Scheduler(SchedulerCause::CannotBlock).to_string()),
        "expected the cannot-block cause, got {err:?}"
    );
    assert!(
        chain(err).contains("cannot block a synchronous task before returning"),
        "the cause carries the words of Wasmtime's `CannotBlockSyncTask`, got {err:?}"
    );
    assert_eq!(
        *released.in_turn.lock().expect("record"),
        Some(true),
        "the destructor ran inside a turn all the same"
    );
    let store = &mut released.store;
    assert!(
        !store.internal().turn_in_flight(),
        "and the turn ended with the failed release"
    );
    assert_eq!(
        store.internal().scheduler().host_task_count(),
        0,
        "the blocked call kept its host task in its own frame, and the \
         failure dropped it rather than leaving it to a later turn"
    );

    let mut after = seen!(store.internal().tables());
    after.context = [0; 2];
    assert_eq!(
        after,
        Seen::default(),
        "nothing of the failed destructor is left in the store"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_host_release_destructor_inside_a_turn() {
    // The destructor of a locally-defined resource is guest code, so
    // the release runs a turn for it: the `probe` the destructor
    // calls first sees a turn in flight, though the host released
    // the handle from outside every turn. The `answer` it calls next
    // is a host `async` import lowered synchronously whose future is
    // ready on its first poll, so the result crosses as the lower
    // returns and the release stands.
    let mut released = host_release_caller(HOST_RELEASE_CALLS_A_HOST_ASYNC_IMPORT, |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                core::future::ready(Ok(x * 2))
            })
            .expect("the registration");
    })
    .await;
    let store = &mut released.store;

    store
        .resource_drop(released.handle)
        .expect("the destructor's import resolved on its first poll");

    assert_eq!(
        *released.in_turn.lock().expect("record"),
        Some(true),
        "a turn of the store was in flight while the destructor ran"
    );
    assert!(
        !store.internal().turn_in_flight(),
        "and the turn ended with the release"
    );

    let mut after = seen!(store.internal().tables());
    after.context = [0; 2];
    assert_eq!(
        after,
        Seen::default(),
        "the destructor's task and its thread ended with the release"
    );

    let answered = released
        .instance
        .get_func("answered")
        .expect("answered export");
    let answered = answered.call(store, &[]).await.expect("call");
    assert_eq!(
        answered.first().cloned(),
        Some(Val::S32(42)),
        "the destructor read the host's result back through the synchronous \
         lower it called the import through"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_host_release_whose_destructor_blocks_on_a_pending_host_task() {
    // The same destructor against an import whose future never
    // resolves. The synchronous lower blocks the destructor's thread
    // through the suspend seam. A destructor may not block — the
    // reference runs it as a synchronous call, and Wasmtime traps a
    // block inside it with `CannotBlockSyncTask` — so the
    // destructor's task holds its instance's may-not-suspend flag
    // and the block fails with the cannot-block cause.
    let mut released =
        host_release_caller(HOST_RELEASE_CALLS_A_HOST_ASYNC_IMPORT, never_answers).await;

    let err = released
        .store
        .resource_drop(released.handle)
        .expect_err("a destructor may not block");

    assert_blocked_release(&mut released, &err);
}

/// A component whose locally-defined resource has a destructor that
/// calls an `async`-typed `answer` import lowered without the
/// `async` option, so the call blocks until the host's future
/// resolves. Its export is `async`-typed and lifted with a callback:
/// it drops an owned handle with `resource.drop`, hands zero to
/// `task.return`, and exits. The export's own task may block, so a
/// block that fails with the cannot-block cause is the destructor's.
const GUEST_DROP_BLOCKS: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $answer' (canon lower (func $answer)))
      (core module $Dtor
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "dtor") (param i32)
          (drop (call $answer (i32.const 21)))))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "answer" (func $answer'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core func $task-return (canon task.return (result u32)))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run") (result i32)
          (call $drop (call $new (i32.const 100)))
          (call $task-return (i32.const 0))
          (i32.const 0))
        (func (export "run-callback") (param i32 i32 i32) (result i32) unreachable))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))
        (export "drop" (func $drop))
        (export "task.return" (func $task-return))))))
      (func (export "run") async (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

#[wcmp_macros::test]
async fn it_fails_a_guest_drop_whose_destructor_blocks_with_the_cannot_block_cause() {
    // A guest's `resource.drop` runs the destructor the way the
    // host's release does, so the destructor's task holds its
    // instance's may-not-suspend flag here too. The export that drops
    // the handle is `async`-typed and may block, so without that flag
    // the seam would fail the block with the stack-switch cause
    // instead, because the host task it waits on is pending.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, GUEST_DROP_BLOCKS)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    never_answers(&mut linker);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let err = run
        .call(&mut store, &[])
        .await
        .expect_err("a destructor may not block, whoever dropped the handle");

    assert!(
        chain(&err).contains(&Error::Scheduler(SchedulerCause::CannotBlock).to_string()),
        "expected the cannot-block cause, got {err:?}"
    );
    assert!(
        chain(&err).contains("cannot block a synchronous task before returning"),
        "the cause carries the words of Wasmtime's `CannotBlockSyncTask`, got {err:?}"
    );
    assert_eq!(
        store.internal().scheduler().host_task_count(),
        0,
        "the blocked call kept its host task in its own frame, and the \
         failure dropped it rather than leaving it to a later turn"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_driver_entered_from_inside_a_destructor() {
    // The release runs the destructor inside a turn, so a driver of
    // the same store that the destructor's host call enters is
    // refused with the recursive-driver cause. The refusal leaves the
    // store untouched and the destructor returns, so the release
    // stands.
    let mut released = host_release_caller(HOST_RELEASE_CALLS_A_HOST_ASYNC_IMPORT, |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                core::future::ready(Ok(x * 2))
            })
            .expect("the registration");
    })
    .await;

    released
        .store
        .resource_drop(released.handle)
        .expect("the destructor returned");

    assert_eq!(
        released.driver.lock().expect("record").clone(),
        Some(Error::Scheduler(SchedulerCause::RecursiveDriver).to_string()),
        "the driver entered from inside the destructor was refused"
    );
    let mut after = seen!(released.store.internal().tables());
    after.context = [0; 2];
    assert_eq!(
        after,
        Seen::default(),
        "the refused driver left nothing behind, and the destructor's task \
         ended with the release"
    );
}

/// A component whose locally-defined resource has a destructor that
/// calls a synchronous `probe` and then an `async`-typed `answer`
/// lowered with the `async` option. The lower does not block: it
/// answers a status word at once, which the destructor keeps, and
/// the result lands in memory at offset 8 whenever the call
/// resolves. The host reads both back through exports.
const HOST_RELEASE_STARTS_A_HOST_ASYNC_IMPORT: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "mem") 1))
      (core instance $libc (instantiate $libc))
      (core func $probe' (canon lower (func $probe)))
      (core func $answer'
        (canon lower (func $answer) async (memory (core memory $libc "mem"))))
      (core module $Dtor
        (import "" "mem" (memory 1))
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (global $status (mut i32) (i32.const -1))
        (func (export "dtor") (param i32)
          (drop (call $probe (i32.const 1)))
          (global.set $status (call $answer (i32.const 21) (i32.const 8))))
        (func (export "status") (result i32) (global.get $status))
        (func (export "answered") (result i32) (i32.load (i32.const 8))))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "mem" (memory $libc "mem"))
        (export "probe" (func $probe'))
        (export "answer" (func $answer'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (func (export "make") (result i32)
          (call $new (i32.const 100))))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))))))
      (export $t "t" (type $r))
      (func (export "make") (result (own $t))
        (canon lift (core func $m "make")))
      (func (export "status") (result s32)
        (canon lift (core func $dtor "status")))
      (func (export "answered") (result s32)
        (canon lift (core func $dtor "answered"))))
    "#
);

#[wcmp_macros::test]
async fn it_leaves_the_host_task_of_an_asynchronously_lowered_import_to_a_later_turn() {
    // An asynchronous lower never blocks, so a destructor may make
    // one. The import's future is pending on the lower's own poll, so
    // the call starts, its host task joins the store, and the release
    // stands. A later turn of a driver polls the task again and
    // lowers its result into the destructor's instance.
    let mut released = host_release_caller(HOST_RELEASE_STARTS_A_HOST_ASYNC_IMPORT, |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                PendingOnce::new(Ok(x * 2))
            })
            .expect("the registration");
    })
    .await;
    let store = &mut released.store;

    store
        .resource_drop(released.handle)
        .expect("an asynchronous lower does not block the destructor");

    assert_eq!(
        *released.in_turn.lock().expect("record"),
        Some(true),
        "the destructor ran inside a turn"
    );
    assert!(
        !store.internal().turn_in_flight(),
        "and the turn ended with the release"
    );
    assert_eq!(
        store.internal().scheduler().host_task_count(),
        1,
        "the started call's host task stayed in the store for a later turn"
    );
    {
        let guard = store.internal().tables().lock().expect("handle tables");
        assert_eq!(
            (
                guard.tasks.scopes().len(),
                guard.tasks.task_count(),
                guard.tasks.thread_count()
            ),
            (0, 0, 0),
            "the destructor's task and its thread ended with the release"
        );
    }

    let instance = &released.instance;
    let status = instance.get_func("status").expect("status export");
    let status = status.call(store, &[]).await.expect("call");
    let Some(Val::S32(status)) = status.first().cloned() else {
        panic!("status answers an s32, got {status:?}");
    };
    assert_eq!(
        status & 0xf,
        1,
        "the lower answered STARTED, with the subtask's index above it"
    );
    assert_ne!(status >> 4, 0, "the started call has a subtask index");

    store
        .run_concurrent(async |_| PendingOnce::new(()).await)
        .await
        .expect("a driver runs the store's turns");

    assert_eq!(
        store.internal().scheduler().host_task_count(),
        0,
        "the driver's turn polled the host task to its end"
    );
    let answered = instance.get_func("answered").expect("answered export");
    let answered = answered.call(store, &[]).await.expect("call");
    assert_eq!(
        answered.first().cloned(),
        Some(Val::S32(42)),
        "the result was lowered into the destructor's instance"
    );
}

/// A component whose export and whose resource's destructor both
/// call one lowered host import, `release`.
///
/// `make` mints a handle for the host. `run` calls `release` with 0,
/// and the destructor calls it with the resource's rep, 100. Both
/// calls go through the same `canon lower`, so they are calls of one
/// host function.
const RELEASE_CALLS_ITSELF_THROUGH_A_DESTRUCTOR: &[u8] = component!(
    r#"
    (component
      (import "release" (func $release (param "x" u32)))
      (core func $release' (canon lower (func $release)))
      (core module $Dtor
        (import "" "release" (func $release (param i32)))
        (func (export "dtor") (param i32)
          (call $release (local.get 0))))
      (core instance $dtor (instantiate $Dtor (with "" (instance
        (export "release" (func $release'))))))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "release" (func $release (param i32)))
        (func (export "make") (result i32)
          (call $new (i32.const 100)))
        (func (export "run")
          (call $release (i32.const 0))))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))
        (export "release" (func $release'))))))
      (export $t "t" (type $r))
      (func (export "make") (result (own $t))
        (canon lift (core func $m "make")))
      (func (export "run")
        (canon lift (core func $m "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_calls_a_host_import_again_from_inside_its_own_call() {
    // The host's `release` drops the handle it holds when the guest
    // passes 0. The drop runs the destructor, and the destructor calls
    // `release` again while the first call of it is still on the
    // stack. Both backends enter a host function at any depth, so the
    // second call returns, the destructor returns, and so does the
    // first call.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, RELEASE_CALLS_ITSELF_THROUGH_A_DESTRUCTOR)
        .await
        .expect("component parses");
    let held: Arc<Mutex<Option<ResourceHandle>>> = Arc::new(Mutex::new(None));
    // Each entry is the argument of a call and how many calls of
    // `release` were on the stack when it started.
    let calls: Arc<Mutex<Vec<(u32, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let depth = Arc::new(Mutex::new(0u32));
    let mut linker: Linker<()> = Linker::new(&engine);
    {
        let held = held.clone();
        let calls = calls.clone();
        linker
            .root()
            .func_wrap(
                "release",
                move |mut call: HostCall<'_, ()>, (x,): (u32,)| -> Result<()> {
                    let entered = {
                        let mut depth = depth.lock().expect("depth");
                        *depth += 1;
                        *depth
                    };
                    calls.lock().expect("calls").push((x, entered));
                    let released = match x {
                        0 => {
                            let handle = held
                                .lock()
                                .expect("held")
                                .take()
                                .expect("the host holds a handle");
                            call.store().internal().resource_drop(handle)
                        }
                        _ => Ok(()),
                    };
                    *depth.lock().expect("depth") -= 1;
                    released
                },
            )
            .expect("the registration");
    }
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let made = instance
        .get_func("make")
        .expect("make export")
        .call(&mut store, &[])
        .await
        .expect("make returned");
    let Some(Val::Own(handle)) = made.first().cloned() else {
        panic!("make returns an owned handle, got {made:?}");
    };
    *held.lock().expect("held") = Some(handle);

    instance
        .get_func("run")
        .expect("run export")
        .call(&mut store, &[])
        .await
        .expect("the call returned");

    assert_eq!(
        *calls.lock().expect("calls"),
        vec![(0, 1), (100, 2)],
        "the destructor called `release` with its rep while the first call \
         of `release` was still on the stack"
    );
}
