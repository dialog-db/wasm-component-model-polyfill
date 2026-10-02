// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for a host task whose own poll poisons the store.
//!
//! A host body reaches the store through its accessor, and what it
//! does there can run guest code: releasing a guest's resource runs
//! the guest's destructor. A destructor that traps poisons the store
//! in the middle of the body's poll, and the trap lets go of every
//! host task the store holds. The task being polled is not held at
//! that moment, and goes when its poll returns, whichever poll it is:
//! the first poll a lower makes, a turn's poll of the woken tasks, or
//! the poll a block makes of its parked call. A task the same turn has
//! out behind it goes unpolled.
//!
//! Releasing a guest's resource from inside a body is the crate's own
//! surface, reached through `StoreContextInternal`. That is why the
//! tests live inside the crate.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::{Pin, pin};
use core::task::{Context, Poll};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::store::StoreContextInternalExt;
use crate::{
    Accessor, Component, Engine, Error, Func, HostCall, Instance, Linker, ResourceHandle, Store,
    StoreContext, TaskCause, Val,
};
use wcmp_macros::component;

/// A component whose resource `r` has a destructor that traps, and
/// whose export `make` mints one.
const TRAPPING_RESOURCE: &[u8] = component!(
    r#"
    (component
      (core module $Dtor
        (func (export "dtor") (param i32) unreachable))
      (core instance $dtor (instantiate $Dtor))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core module $M
        (import "" "new" (func $new (param i32) (result i32)))
        (func (export "make") (result i32)
          (call $new (i32.const 13))))
      (core instance $m (instantiate $M (with "" (instance
        (export "new" (func $new))))))
      (export $t "r" (type $r))
      (func (export "make") (result (own $t))
        (canon lift (core func $m "make"))))
    "#
);

/// A component that calls the host's `async` functions `boom` and
/// `pend`.
///
/// - `pend-then-boom` and `boom-then-pend` are lifted `async` with a
///   callback. Each calls both functions through an asynchronous
///   lower, in the order its name gives, joins both subtasks to a
///   set, and waits on the set. The callback calls `tick` and ends
///   the task.
/// - `boom-sync` is lifted synchronously and calls `boom` through a
///   synchronous lower, so it blocks until the call returns.
/// - `tick-sync` is lifted synchronously and calls `tick`.
const CALLER: &[u8] = component!(
    r#"
    (component
      (import "boom" (func $boom async))
      (import "pend" (func $pend async))
      (import "tick" (func $tick))
      (core func $boom-async (canon lower (func $boom) async))
      (core func $boom-sync (canon lower (func $boom)))
      (core func $pend-async (canon lower (func $pend) async))
      (core func $tick' (canon lower (func $tick)))
      (core func $task-return (canon task.return))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $M
        (import "" "boom-async" (func $boom-async (result i32)))
        (import "" "boom-sync" (func $boom-sync))
        (import "" "pend-async" (func $pend-async (result i32)))
        (import "" "tick" (func $tick))
        (import "" "task.return" (func $task-return))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func $wait-on (param $set i32) (param $status i32)
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set)))
        (func (export "pend-then-boom") (result i32)
          (local $set i32)
          (local.set $set (call $set-new))
          (call $wait-on (local.get $set) (call $pend-async))
          (call $wait-on (local.get $set) (call $boom-async))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "boom-then-pend") (result i32)
          (local $set i32)
          (local.set $set (call $set-new))
          (call $wait-on (local.get $set) (call $boom-async))
          (call $wait-on (local.get $set) (call $pend-async))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "callback") (param i32 i32 i32) (result i32)
          (call $tick)
          (call $task-return)
          (i32.const 0))
        (func (export "boom-sync")
          (call $boom-sync))
        (func (export "tick-sync")
          (call $tick)))
      (core instance $m (instantiate $M (with "" (instance
        (export "boom-async" (func $boom-async))
        (export "boom-sync" (func $boom-sync))
        (export "pend-async" (func $pend-async))
        (export "tick" (func $tick'))
        (export "task.return" (func $task-return))
        (export "waitable-set.new" (func $set-new))
        (export "waitable.join" (func $join))))))
      (func (export "pend-then-boom") async
        (canon lift (core func $m "pend-then-boom") async
          (callback (core func $m "callback"))))
      (func (export "boom-then-pend") async
        (canon lift (core func $m "boom-then-pend") async
          (callback (core func $m "callback"))))
      (func (export "boom-sync")
        (canon lift (core func $m "boom-sync")))
      (func (export "tick-sync")
        (canon lift (core func $m "tick-sync"))))
    "#
);

/// The message of Wasmtime's cannot-enter trap.
const CANNOT_ENTER: &str = "cannot enter component instance";

/// What the host saw of one of its futures.
#[derive(Clone, Default)]
struct Seen {
    /// How many times the future was polled.
    polls: Arc<AtomicU32>,
    /// Whether the future was dropped.
    dropped: Arc<AtomicBool>,
}

impl Seen {
    fn polls(&self) -> u32 {
        self.polls.load(Ordering::Relaxed)
    }

    fn dropped(&self) -> bool {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// The future of `pend`: pending for good, and noting its polls and
/// its drop.
struct Pend(Seen);

impl Future for Pend {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.polls.fetch_add(1, Ordering::Relaxed);
        Poll::Pending
    }
}

impl Drop for Pend {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::Relaxed);
    }
}

/// What every `boom` shares with the test.
#[derive(Clone, Default)]
struct Plan {
    /// The poll, counted from 1, on which `boom` releases the handle.
    at: Arc<AtomicU32>,
    /// The guest's resource `boom` or `tick` releases.
    handle: Arc<Mutex<Option<ResourceHandle>>>,
    /// What the release answered, once `boom` or `tick` made it.
    released: Arc<Mutex<Option<Result<(), String>>>>,
    /// Whether `boom` answers once it has released the handle, rather
    /// than staying pending.
    answers: Arc<AtomicBool>,
    /// Whether `tick` releases the handle before it returns.
    tick_releases: Arc<AtomicBool>,
}

impl Plan {
    /// Release the guest's resource in `store`, when the plan still
    /// holds it, and note what the release answered.
    fn release(&self, store: &mut StoreContext<'_, ()>) {
        if let Some(handle) = self.handle.lock().expect("the handle").take() {
            let released = store
                .internal()
                .resource_drop(handle)
                .map_err(|error| error.to_string());
            *self.released.lock().expect("the release") = Some(released);
        }
    }
}

/// The future of `boom`: pending for good, unless the plan says it
/// answers. On the poll the plan names it releases the guest's
/// resource through its accessor, and the guest's destructor traps.
struct Boom {
    accessor: Accessor<()>,
    plan: Plan,
    seen: Seen,
}

impl Future for Boom {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        let polls = self.seen.polls.fetch_add(1, Ordering::Relaxed) + 1;
        if polls == self.plan.at.load(Ordering::Relaxed) {
            if let Err(error) = self.accessor.with(|store| self.plan.release(store)) {
                *self.plan.released.lock().expect("the release") = Some(Err(error.to_string()));
            }
            if self.plan.answers.load(Ordering::Relaxed) {
                return Poll::Ready(Ok(()));
            }
        }
        Poll::Pending
    }
}

impl Drop for Boom {
    fn drop(&mut self) {
        self.seen.dropped.store(true, Ordering::Relaxed);
    }
}

/// A store with both components instantiated, and the guest's
/// resource `boom` or `tick` releases.
struct Fixture {
    store: Store<()>,
    caller: Instance,
    make: Func,
    plan: Plan,
    boom: Seen,
    pend: Seen,
    ticks: Arc<AtomicU32>,
}

impl Fixture {
    /// The fixture, with `boom` set to release the resource on its
    /// poll `at`.
    async fn new(at: u32) -> Self {
        let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
        let resource = Component::new(&engine, TRAPPING_RESOURCE)
            .await
            .expect("the resource component parses");
        let caller = Component::new(&engine, CALLER)
            .await
            .expect("the caller component parses");
        let plan = Plan::default();
        plan.at.store(at, Ordering::Relaxed);
        let (boom, pend) = (Seen::default(), Seen::default());
        let ticks = Arc::new(AtomicU32::new(0));
        let mut linker: Linker<()> = Linker::new(&engine);
        let mut root = linker.root();
        let (booms, plans) = (boom.clone(), plan.clone());
        root.func_wrap_concurrent("boom", move |accessor: &Accessor<()>, (): ()| Boom {
            accessor: accessor.clone(),
            plan: plans.clone(),
            seen: booms.clone(),
        })
        .expect("the registration of `boom`");
        let pends = pend.clone();
        root.func_wrap_concurrent("pend", move |_accessor: &Accessor<()>, (): ()| {
            Pend(pends.clone())
        })
        .expect("the registration of `pend`");
        let (tick, ticking) = (ticks.clone(), plan.clone());
        root.func_wrap("tick", move |mut call: HostCall<'_, ()>, (): ()| {
            tick.fetch_add(1, Ordering::Relaxed);
            if ticking.tick_releases.load(Ordering::Relaxed) {
                ticking.release(call.store());
            }
            Ok(())
        })
        .expect("the registration of `tick`");
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let resource = linker
            .instantiate(&mut store, &resource)
            .await
            .expect("the resource component instantiates");
        let caller = linker
            .instantiate(&mut store, &caller)
            .await
            .expect("the caller component instantiates");
        let make = resource.get_func("make").expect("`make` is exported");
        let made = make.call(&mut store, &[]).await.expect("`make` mints one");
        let Some(Val::Own(handle)) = made.first().cloned() else {
            panic!("`make` answers with an owned handle, got {made:?}");
        };
        *plan.handle.lock().expect("the handle") = Some(handle);
        Self {
            store,
            caller,
            make,
            plan,
            boom,
            pend,
            ticks,
        }
    }

    /// The caller's export `name`.
    fn func(&self, name: &str) -> Func {
        self.caller
            .get_func(name)
            .unwrap_or_else(|| panic!("`{name}` is exported"))
    }

    /// Assert that `boom` released the resource and the destructor
    /// trapped, rather than the release being refused.
    fn assert_destructor_trapped(&self) {
        let released = self.plan.released.lock().expect("the release").clone();
        let Some(Err(message)) = released else {
            panic!("`boom` released the resource and the destructor trapped, got {released:?}");
        };
        assert!(
            !message.contains(CANNOT_ENTER),
            "the destructor ran and trapped, rather than being refused, got {message}"
        );
    }

    /// Assert that a trap poisoned the store: a call of `make` fails
    /// with the cannot-enter cause.
    async fn assert_poisoned(&mut self) {
        let error = self
            .make
            .call(&mut self.store, &[])
            .await
            .expect_err("a poisoned store refuses the call");
        assert!(
            matches!(error, Error::Task(TaskCause::CannotEnter)),
            "the call fails with the cannot-enter cause, got {error:?}"
        );
    }
}

/// Assert that the guest's call failed because its host call's first
/// poll poisoned the store.
fn assert_refused_after_poison(error: &Error) {
    let described = format!("{error:?} {error}");
    assert!(
        described.contains("CannotEnter") || described.contains(CANNOT_ENTER),
        "the guest's call fails with the cannot-enter cause, got {described}"
    );
}

#[wcmp_macros::test]
async fn it_drops_an_asynchronous_host_call_whose_first_poll_poisons_the_store() {
    let mut fixture = Fixture::new(1).await;
    let call = fixture.func("pend-then-boom");

    // The guest's call fails on the stack, which is a trap of the task
    // that made it, and the trap ends the entry that was polling.
    let error = match fixture
        .store
        .run_concurrent(async |accessor| call.call_concurrent(accessor, &[]).await)
        .await
    {
        Ok(_) => panic!("the trap ends the entry around the call"),
        Err(error) => error,
    };

    fixture.assert_destructor_trapped();
    assert_refused_after_poison(&error);
    assert!(
        fixture.boom.dropped(),
        "the future whose first poll poisoned the store was dropped rather than kept"
    );
    assert!(
        fixture.pend.dropped(),
        "the pending future the store held was dropped at the trap"
    );
    assert_eq!(fixture.boom.polls(), 1, "`boom` was polled once");
    assert_eq!(fixture.pend.polls(), 1, "`pend` was polled once");
    assert_eq!(
        fixture.ticks.load(Ordering::Relaxed),
        0,
        "the callback never ran"
    );
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_drops_the_polled_host_task_and_the_woken_one_behind_it_when_a_poll_poisons_the_store() {
    // Both calls start, and both tasks count as woken, so the turn
    // after they started has both out. `boom` is polled first and
    // poisons the store; `pend` behind it goes unpolled.
    let mut fixture = Fixture::new(2).await;
    let call = fixture.func("boom-then-pend");
    let (boom, pend) = (fixture.boom.clone(), fixture.pend.clone());

    fixture
        .store
        .run_concurrent(async |accessor| {
            // The call never returns: its task waits on subtasks the
            // trap let go of. It is polled until both futures are
            // gone, or for long enough that they would have been.
            let mut called = pin!(call.call_concurrent(accessor, &[]));
            let mut polls = 0;
            poll_fn(|context| {
                polls += 1;
                if called.as_mut().poll(context).is_ready() {
                    return Poll::Ready(());
                }
                if (boom.dropped() && pend.dropped()) || polls == 64 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        })
        .await
        .expect("the entry around the call returns");

    fixture.assert_destructor_trapped();
    assert!(
        fixture.boom.dropped(),
        "the future whose poll poisoned the store was dropped"
    );
    assert_eq!(
        fixture.boom.polls(),
        2,
        "`boom` was polled as its call started and once by a turn, and never again"
    );
    assert!(
        fixture.pend.dropped(),
        "the future the turn had out behind it was dropped"
    );
    assert_eq!(
        fixture.pend.polls(),
        1,
        "`pend` was polled as its call started, and the turn that had it out when the \
         store was poisoned did not poll it"
    );
    assert_eq!(
        fixture.ticks.load(Ordering::Relaxed),
        0,
        "the callback never ran"
    );
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_drops_a_synchronous_host_call_whose_first_poll_poisons_the_store() {
    let mut fixture = Fixture::new(1).await;
    let call = fixture.func("boom-sync");

    let error = call
        .call(&mut fixture.store, &[])
        .await
        .expect_err("the guest's call fails once the store is poisoned");

    fixture.assert_destructor_trapped();
    assert_refused_after_poison(&error);
    assert!(
        fixture.boom.dropped(),
        "the future whose first poll poisoned the store was dropped rather than parked"
    );
    assert_eq!(fixture.boom.polls(), 1, "`boom` was polled once");
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_drops_a_parked_host_call_whose_later_poll_poisons_the_store() {
    // The first poll parks the call, and the block polls it again,
    // which poisons the store. The block then fails, as a call whose
    // future is gone can never return.
    let mut fixture = Fixture::new(2).await;
    let call = fixture.func("boom-sync");

    call.call(&mut fixture.store, &[])
        .await
        .expect_err("the guest's call fails once the store is poisoned");

    fixture.assert_destructor_trapped();
    assert!(
        fixture.boom.dropped(),
        "the parked future whose poll poisoned the store was dropped"
    );
    assert_eq!(
        fixture.boom.polls(),
        2,
        "`boom` was polled as the call started and once by the block, and never again"
    );
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_fails_an_asynchronous_host_call_whose_first_poll_poisons_the_store_and_answers() {
    // The body answers, but the poll that answered ran a destructor
    // that trapped. The guest that would take the answer is guest code
    // a poisoned store does not run, so its call fails instead.
    let mut fixture = Fixture::new(1).await;
    fixture.plan.answers.store(true, Ordering::Relaxed);
    let call = fixture.func("pend-then-boom");

    let error = match fixture
        .store
        .run_concurrent(async |accessor| call.call_concurrent(accessor, &[]).await)
        .await
    {
        Ok(_) => panic!("the trap ends the entry around the call"),
        Err(error) => error,
    };

    fixture.assert_destructor_trapped();
    assert_refused_after_poison(&error);
    assert!(
        fixture.boom.dropped(),
        "the future that answered was dropped, and its answer crossed into no guest"
    );
    assert_eq!(
        fixture.ticks.load(Ordering::Relaxed),
        0,
        "the callback never ran"
    );
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_fails_a_synchronous_host_call_whose_first_poll_poisons_the_store_and_answers() {
    let mut fixture = Fixture::new(1).await;
    fixture.plan.answers.store(true, Ordering::Relaxed);
    let call = fixture.func("boom-sync");

    let error = call
        .call(&mut fixture.store, &[])
        .await
        .expect_err("the guest's call fails although the body answered");

    fixture.assert_destructor_trapped();
    assert_refused_after_poison(&error);
    assert_eq!(fixture.boom.polls(), 1, "`boom` was polled once");
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_fails_the_guests_call_when_a_synchronous_host_function_poisons_the_store_mid_body() {
    // `tick` releases the guest's resource, whose destructor traps,
    // and then returns as though nothing happened. The guest that
    // called it is guest code a poisoned store does not run, so the
    // call fails with the cannot-enter cause.
    let mut fixture = Fixture::new(0).await;
    fixture.plan.tick_releases.store(true, Ordering::Relaxed);
    let call = fixture.func("tick-sync");

    let error = call
        .call(&mut fixture.store, &[])
        .await
        .expect_err("the guest's call fails although the host function returned");

    fixture.assert_destructor_trapped();
    assert_refused_after_poison(&error);
    assert_eq!(
        fixture.ticks.load(Ordering::Relaxed),
        1,
        "`tick` ran once, and returned"
    );
    fixture.assert_poisoned().await;
}

#[wcmp_macros::test]
async fn it_fails_a_pending_concurrent_call_with_the_cannot_enter_cause_after_the_closure_poisons_the_store()
 {
    // The trap happens outside any turn, in a destructor the closure
    // releases through its accessor. It discards the task's work, so
    // the call's future has nothing left to wait for.
    let mut fixture = Fixture::new(0).await;
    let call = fixture.func("pend-then-boom");
    let plan = fixture.plan.clone();

    let answered = fixture
        .store
        .run_concurrent(async |accessor| {
            let mut called = pin!(call.call_concurrent(accessor, &[]));
            let first =
                poll_fn(|context| Poll::Ready(called.as_mut().poll(context).is_ready())).await;
            assert!(!first, "the call is pending before the store is poisoned");
            accessor
                .with(|store| plan.release(store))
                .expect("the closure reaches the store");
            called.await
        })
        .await
        .expect("the trap met no turn, so the entry returns what the closure answered");

    fixture.assert_destructor_trapped();
    let Err(error) = answered else {
        panic!("the call fails once the store is poisoned, got {answered:?}");
    };
    assert!(
        matches!(error, Error::Task(TaskCause::CannotEnter)),
        "the call fails with the cannot-enter cause, got {error:?}"
    );
    fixture.assert_poisoned().await;
}
