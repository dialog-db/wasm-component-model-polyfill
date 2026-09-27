//! Baseline tests for the poisoned store.
//!
//! A trap poisons the store, as Wasmtime keeps one trapped flag per
//! store, and a poisoned store runs no more guest code. Every host
//! entry into a guest fails with Wasmtime's cannot-enter trap, "cannot
//! enter component instance": a call, a concurrent call, an
//! instantiation, and the release of a resource a guest defines. What
//! runs no guest code still works: the release of a resource the host
//! defines, and a `run_concurrent` whose closure does only host work.
//!
//! The tests drive one component with an export for each trigger and
//! read the poison back through the one entry every test shares, a
//! call of `ok`, which returns 7 in a store no trap has poisoned.
//!
//! At the moment of the trap the store also discards its work: every
//! queued guest work item, and every pending host future, each of
//! which is dropped there. Two tests read that back: one through a
//! component of its own, whose tasks leave both kinds of work behind,
//! and one through a pipe of the host's own that starts after the
//! trap, which touches no guest and still runs. A failure of such a
//! pipe is no trap: it ends the entry whose turn met it, and leaves
//! the store usable.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::{Pin, pin};
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Accessor, Component, Engine, Error, FunctionType, FutureConsumer, FutureReader, HostCall,
    Instance, Linker, Module, ResourceHandle, ResourceTypeId, SchedulerCause, Source, Store,
    StoreContext, TaskCause, Val,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component with one export per trigger of the poisoned store, and
/// `ok`, which runs no trap at all.
///
/// - `unreachable` traps in its own core code.
/// - `builtin-trap` drops a handle index its table never gave out,
///   which traps inside the `resource.drop` built-in.
/// - `host-fail` calls the host's `fail` through a synchronous lower,
///   and the host function fails.
/// - `take` takes a `u32`, so a host [`Val`] of another type fails
///   as the call lowers it.
/// - `make` mints a resource of the component's own type `r`, whose
///   destructor traps for the rep 13 and returns for any other.
///
/// The component also imports the host's resource `thing`, so that
/// the host's destructor for it is the store's to run.
const TRIGGERS: &[u8] = component!(
    r#"
    (component
      (import "fail" (func $fail))
      (import "thing" (type $thing (sub resource)))
      (core func $fail' (canon lower (func $fail)))
      (core module $Dtor
        (func (export "dtor") (param i32)
          (if (i32.eq (local.get 0) (i32.const 13))
            (then unreachable))))
      (core instance $dtor (instantiate $Dtor))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $M
        (import "" "fail" (func $fail))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "ok") (result i32) (i32.const 7))
        (func (export "unreachable") unreachable)
        (func (export "builtin-trap") (call $drop (i32.const 99)))
        (func (export "host-fail") (call $fail))
        (func (export "take") (param i32))
        (func (export "make") (param i32) (result i32)
          (call $new (local.get 0))))
      (core instance $m (instantiate $M (with "" (instance
        (export "fail" (func $fail'))
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (export $t "r" (type $r))
      (func (export "ok") (result u32)
        (canon lift (core func $m "ok")))
      (func (export "unreachable")
        (canon lift (core func $m "unreachable")))
      (func (export "builtin-trap")
        (canon lift (core func $m "builtin-trap")))
      (func (export "host-fail")
        (canon lift (core func $m "host-fail")))
      (func (export "take") (param "x" u32)
        (canon lift (core func $m "take")))
      (func (export "make") (param "rep" u32) (result (own $t))
        (canon lift (core func $m "make"))))
    "#
);

/// A component whose one core module traps in its `start` function.
const TRAPS_IN_START: &[u8] = component!(
    r#"
    (component
      (core module $M
        (func $start unreachable)
        (start $start))
      (core instance (instantiate $M)))
    "#
);

/// A component that runs no guest code as it instantiates, for the
/// instantiation a poisoned store refuses.
const EMPTY: &[u8] = component!(
    r#"
    (component
      (core module $M (func (export "f")))
      (core instance (instantiate $M)))
    "#
);

/// A core module whose `start` function traps.
const MODULE_TRAPS_IN_START: &[u8] = wasm!(
    r#"
    (module
      (func $start unreachable)
      (start $start))
    "#
);

/// A core module with no `start` function, which runs no guest code
/// as it instantiates.
const MODULE_WITHOUT_START: &[u8] = wasm!(
    r#"
    (module (func (export "f")))
    "#
);

/// The message of Wasmtime's cannot-enter trap, written out so that a
/// change to the words is a change a test is told about.
const CANNOT_ENTER: &str = "cannot enter component instance";

/// The host data of every store: how many times the host's
/// destructor for `thing` ran.
#[derive(Default)]
struct Drops(Arc<AtomicU32>);

/// A store with [`TRIGGERS`] instantiated in it, the linker that
/// instantiated it, and the type of the host's resource `thing`.
struct Fixture {
    engine: Engine,
    linker: Linker<Drops>,
    store: Store<Drops>,
    instance: Instance,
    thing: ResourceTypeId,
    drops: Arc<AtomicU32>,
}

impl Fixture {
    async fn new() -> Self {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, TRIGGERS)
            .await
            .expect("component parses");
        let mut linker: Linker<Drops> = Linker::new(&engine);
        let mut root = linker.root();
        root.func_new(
            "fail",
            FunctionType {
                parameters: Vec::new(),
                result: None,
                async_: false,
            },
            |_: HostCall<'_, Drops>, _args, _results| {
                Err(Error::Internal {
                    message: "the host function failed".to_owned(),
                })
            },
        )
        .expect("the registration of `fail`");
        let thing = root
            .resource("thing", |data: &mut Drops, _rep: u32| {
                data.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .expect("the registration of `thing`");
        let drops = Arc::new(AtomicU32::new(0));
        let mut store = Store::new(&engine, Drops(drops.clone())).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("the component instantiates");
        Self {
            engine,
            linker,
            store,
            instance,
            thing,
            drops,
        }
    }

    /// Call the export `name` with `args`.
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Box<[Val]>, Error> {
        let func = self
            .instance
            .get_func(name)
            .unwrap_or_else(|| panic!("`{name}` is exported"));
        func.call(&mut self.store, args).await
    }

    /// A resource of the component's own type, minted by `make`.
    async fn make(&mut self, rep: u32) -> ResourceHandle {
        let made = self
            .call("make", &[Val::U32(rep)])
            .await
            .expect("`make` mints a resource");
        let Some(Val::Own(handle)) = made.first().cloned() else {
            panic!("`make` answers with an owned handle, got {made:?}");
        };
        handle
    }

    /// Assert that a call of `ok` answers 7, so the store is usable.
    async fn assert_usable(&mut self, after: &str) {
        let answered = self
            .call("ok", &[])
            .await
            .unwrap_or_else(|error| panic!("the store is usable after {after}, got {error}"));
        assert_eq!(answered.first(), Some(&Val::U32(7)), "after {after}");
    }

    /// Assert that a call of `ok` fails with the cannot-enter cause,
    /// so a trap poisoned the store.
    async fn assert_poisoned(&mut self, after: &str) {
        let error = self
            .call("ok", &[])
            .await
            .expect_err("a poisoned store refuses the call");
        assert_cannot_enter(&error, &format!("a call after {after}"));
    }
}

/// Assert that `error` is the cannot-enter cause, with its message.
fn assert_cannot_enter(error: &Error, entry: &str) {
    assert!(
        matches!(error, Error::Task(TaskCause::CannotEnter)),
        "{entry} fails with the cannot-enter cause, got {error:?}"
    );
    assert!(
        error.to_string().contains(CANNOT_ENTER),
        "{entry} fails with Wasmtime's message, got {error}"
    );
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_guest_trap() {
    let mut fixture = Fixture::new().await;
    fixture.assert_usable("instantiation").await;

    fixture
        .call("unreachable", &[])
        .await
        .expect_err("the guest traps");

    fixture.assert_poisoned("a guest trap").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_built_in_trap() {
    let mut fixture = Fixture::new().await;

    fixture
        .call("builtin-trap", &[])
        .await
        .expect_err("`resource.drop` of a handle the table never gave out traps");

    fixture.assert_poisoned("a built-in trap").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_failed_host_function_under_a_synchronous_lower() {
    let mut fixture = Fixture::new().await;

    let error = fixture
        .call("host-fail", &[])
        .await
        .expect_err("the host function fails the guest's call");
    assert!(
        format!("{error:?}").contains("the host function failed"),
        "the call reports the host function's failure, got {error:?}"
    );

    fixture.assert_poisoned("a failed host function").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_host_value_of_the_wrong_type() {
    // Wasmtime lowers the arguments after its entry check, so a value
    // of the wrong type fails a call that has already entered the
    // guest's store, and poisons it. The polyfill matches that.
    let mut fixture = Fixture::new().await;

    let error = fixture
        .call("take", &[Val::String("seven".into())])
        .await
        .expect_err("a string is not a `u32`");
    assert!(
        matches!(error, Error::Abi(_)),
        "the lowering refuses the value, got {error:?}"
    );

    fixture
        .assert_poisoned("a host value of the wrong type")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_components_core_start_function() {
    let mut fixture = Fixture::new().await;
    let traps = Component::new(&fixture.engine, TRAPS_IN_START)
        .await
        .expect("component parses");

    let error = match fixture.linker.instantiate(&mut fixture.store, &traps).await {
        Ok(_) => panic!("a start function that traps fails the instantiation"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Instantiation(_)),
        "the start function's trap fails the instantiation, got {error:?}"
    );

    fixture
        .assert_poisoned("a trap in a component's core start function")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_host_modules_start_function() {
    let mut fixture = Fixture::new().await;
    let module = Module::new(&fixture.engine, MODULE_TRAPS_IN_START)
        .await
        .expect("the module compiles");

    module
        .instantiate(&mut fixture.store, &[])
        .await
        .expect_err("a start function that traps fails the instantiation");

    fixture
        .assert_poisoned("a trap in a host module's start function")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_destructor() {
    let mut fixture = Fixture::new().await;
    let handle = fixture.make(13).await;

    fixture
        .store
        .resource_drop(handle)
        .expect_err("the destructor traps for the rep 13");

    fixture.assert_poisoned("a trap in a destructor").await;
}

#[wcmp_macros::test]
async fn it_leaves_the_store_usable_after_a_destructor_that_returns() {
    // The release that poisons above differs from this one only in
    // the rep, so the trap is what poisons, not the release.
    let mut fixture = Fixture::new().await;
    let handle = fixture.make(5).await;

    fixture
        .store
        .resource_drop(handle)
        .expect("the destructor returns for the rep 5");

    fixture.assert_usable("a destructor that returned").await;
}

#[wcmp_macros::test]
async fn it_refuses_every_guest_entry_of_a_poisoned_store() {
    let mut fixture = Fixture::new().await;
    // A resource of each kind the host holds, minted while the store
    // is still usable.
    let guest_resource = fixture.make(5).await;
    let host_resource = fixture
        .store
        .resource_new(fixture.thing, 42)
        .expect("the host mints its own resource");
    let ok = fixture.instance.get_func("ok").expect("`ok` is exported");
    let typed = fixture
        .instance
        .get_func("ok")
        .expect("`ok` is exported")
        .typed::<(), u32>()
        .expect("`ok` is `() -> u32`");

    fixture
        .call("unreachable", &[])
        .await
        .expect_err("the guest traps");

    // A call, untyped and typed.
    let error = ok
        .call(&mut fixture.store, &[])
        .await
        .expect_err("a poisoned store refuses `Func::call`");
    assert_cannot_enter(&error, "`Func::call`");
    let error = typed
        .call(&mut fixture.store, ())
        .await
        .expect_err("a poisoned store refuses `TypedFunc::call`");
    assert_cannot_enter(&error, "`TypedFunc::call`");

    // A concurrent call, untyped and typed. The closure runs, and the
    // refusal is the call's own: the entry around it returns.
    let (untyped, typed_concurrent) = fixture
        .store
        .run_concurrent(async |accessor| {
            let untyped = ok.call_concurrent(accessor, &[]).await;
            let typed_concurrent = typed.call_concurrent(accessor, ()).await;
            (untyped, typed_concurrent)
        })
        .await
        .expect("the entry around the concurrent calls returns");
    assert_cannot_enter(
        &untyped.expect_err("a poisoned store refuses `Func::call_concurrent`"),
        "`Func::call_concurrent`",
    );
    assert_cannot_enter(
        &typed_concurrent.expect_err("a poisoned store refuses `TypedFunc::call_concurrent`"),
        "`TypedFunc::call_concurrent`",
    );

    // An instantiation, of a component and of a core module, each of
    // which would run no guest code of its own.
    let empty = Component::new(&fixture.engine, EMPTY)
        .await
        .expect("component parses");
    let error = match fixture.linker.instantiate(&mut fixture.store, &empty).await {
        Ok(_) => panic!("a poisoned store refuses `Linker::instantiate`"),
        Err(error) => error,
    };
    assert_cannot_enter(&error, "`Linker::instantiate`");
    let module = Module::new(&fixture.engine, MODULE_WITHOUT_START)
        .await
        .expect("the module compiles");
    let error = module
        .instantiate(&mut fixture.store, &[])
        .await
        .expect_err("a poisoned store refuses a core module's instantiation");
    assert_cannot_enter(&error, "`Module::instantiate`");

    // The release of a resource the guest defines runs guest code;
    // the release of the host's own runs the host's destructor.
    let error = fixture
        .store
        .resource_drop(guest_resource)
        .expect_err("a poisoned store refuses the release of a guest's resource");
    assert_cannot_enter(&error, "`Store::resource_drop` of a guest's resource");
    fixture
        .store
        .resource_drop(host_resource)
        .expect("the release of the host's resource runs no guest code");
    assert_eq!(
        fixture.drops.load(Ordering::Relaxed),
        1,
        "the host's destructor ran once"
    );

    // A closure that does only host work runs to its value, and the
    // host data is the store's still.
    let value = fixture
        .store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| store.data().0.load(Ordering::Relaxed) + 41)
                .expect("the closure reaches the host data")
        })
        .await
        .expect("a closure that does only host work runs in a poisoned store");
    assert_eq!(value, 42, "the entry returns what the closure returned");
    fixture.store.data_mut().0.fetch_add(1, Ordering::Relaxed);
    assert_eq!(
        fixture.store.data().0.load(Ordering::Relaxed),
        2,
        "the host data is read and written as before"
    );
}

/// A component with three exports, each the whole of one task of the
/// test that follows.
///
/// - `hold` is lifted `async` with a callback. It calls the host
///   `async` function `pend` through an asynchronous lower, joins the
///   subtask to a set, and waits on the set. Its callback calls `tick`
///   with 2 and would end the task.
/// - `spin` is lifted `async` with a callback. It and its callback
///   each call `tick` with 1 and give way, so a callback of its task
///   is always queued.
/// - `trap` traps in its own core code.
const DISCARDS: &[u8] = component!(
    r#"
    (component
      (import "pend" (func $pend async))
      (import "tick" (func $tick (param "who" u32)))
      (core func $pend' (canon lower (func $pend) async))
      (core func $tick' (canon lower (func $tick)))
      (core func $task-return (canon task.return))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $M
        (import "" "pend" (func $pend (result i32)))
        (import "" "tick" (func $tick (param i32)))
        (import "" "task.return" (func $task-return))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "hold") (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $pend))
          (local.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "hold-callback") (param i32 i32 i32) (result i32)
          (call $tick (i32.const 2))
          (call $task-return)
          (i32.const 0))
        (func (export "spin") (result i32)
          (call $tick (i32.const 1))
          (i32.const 1))
        (func (export "spin-callback") (param i32 i32 i32) (result i32)
          (call $tick (i32.const 1))
          (i32.const 1))
        (func (export "trap") unreachable))
      (core instance $m (instantiate $M (with "" (instance
        (export "pend" (func $pend'))
        (export "tick" (func $tick'))
        (export "task.return" (func $task-return))
        (export "waitable-set.new" (func $set-new))
        (export "waitable.join" (func $join))))))
      (func (export "hold") async
        (canon lift (core func $m "hold") async
          (callback (core func $m "hold-callback"))))
      (func (export "spin") async
        (canon lift (core func $m "spin") async
          (callback (core func $m "spin-callback"))))
      (func (export "trap")
        (canon lift (core func $m "trap"))))
    "#
);

/// What the host saw of the guest's work and of its own future.
#[derive(Clone, Default)]
struct Seen {
    /// How many times `spin`'s task called `tick`.
    spins: Arc<AtomicU32>,
    /// How many times `hold`'s callback called `tick`.
    holds: Arc<AtomicU32>,
    /// Whether the future of `pend` was polled.
    started: Arc<AtomicBool>,
    /// Whether the future of `pend` may complete, which it does on
    /// the first poll after this is set.
    release: Arc<AtomicBool>,
    /// The waker the future of `pend` was last polled with.
    waker: Arc<Mutex<Option<Waker>>>,
    /// Whether the future of `pend` was dropped.
    dropped: Arc<AtomicBool>,
    /// What the future's reach into the store answered as it dropped,
    /// once it has dropped.
    reach: Arc<Mutex<Option<Result<(), String>>>>,
}

/// The future of `pend`: pending until the test releases it, and
/// noting its drop.
///
/// Its `Drop` reaches the store through the accessor it was handed,
/// as a future that closes a stream as it drops does, and records
/// what the reach answered.
struct Pend {
    accessor: Accessor<()>,
    seen: Seen,
}

impl Future for Pend {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.seen.started.store(true, Ordering::Relaxed);
        if self.seen.release.load(Ordering::Relaxed) {
            return Poll::Ready(Ok(()));
        }
        *self.seen.waker.lock().expect("the waker") = Some(context.waker().clone());
        Poll::Pending
    }
}

impl Drop for Pend {
    fn drop(&mut self) {
        let reach = self
            .accessor
            .with(|_store| ())
            .map_err(|error| error.to_string());
        if let Ok(mut slot) = self.seen.reach.lock() {
            *slot = Some(reach);
        }
        self.seen.dropped.store(true, Ordering::Relaxed);
    }
}

#[wcmp_macros::test]
async fn it_discards_queued_guest_work_and_drops_host_futures_when_a_trap_poisons_the_store() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, DISCARDS)
        .await
        .expect("component parses");
    let seen = Seen::default();
    let mut linker: Linker<()> = Linker::new(&engine);
    let mut root = linker.root();
    let ticks = seen.clone();
    root.func_wrap("tick", move |_call: HostCall<'_, ()>, (who,): (u32,)| {
        match who {
            1 => ticks.spins.fetch_add(1, Ordering::Relaxed),
            _ => ticks.holds.fetch_add(1, Ordering::Relaxed),
        };
        Ok(())
    })
    .expect("the registration of `tick`");
    let pends = seen.clone();
    root.func_wrap_concurrent("pend", move |accessor: &Accessor<()>, (): ()| Pend {
        accessor: accessor.clone(),
        seen: pends.clone(),
    })
    .expect("the registration of `pend`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    let hold = instance.get_func("hold").expect("`hold` is exported");
    let spin = instance.get_func("spin").expect("`spin` is exported");
    let trap = instance.get_func("trap").expect("`trap` is exported");

    let entry = store
        .run_concurrent(async |accessor| {
            // The first task starts the host call and waits on it, and
            // the second gives way for good. Neither call ever
            // returns, so each is polled until the host future has
            // started and the second task has run its callback.
            let mut held = pin!(hold.call_concurrent(accessor, &[]));
            let mut spun = pin!(spin.call_concurrent(accessor, &[]));
            poll_fn(|context| {
                assert!(held.as_mut().poll(context).is_pending(), "`hold` waits");
                assert!(spun.as_mut().poll(context).is_pending(), "`spin` gives way");
                if seen.started.load(Ordering::Relaxed) && seen.spins.load(Ordering::Relaxed) >= 2 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
            assert!(
                !seen.dropped.load(Ordering::Relaxed),
                "the host future is alive before the trap"
            );

            // The third task traps, which poisons the store and ends
            // the entry: the closure goes no further than this.
            trap.call_concurrent(accessor, &[]).await
        })
        .await;

    let error = match entry {
        Ok(called) => panic!("the trap ends the entry, and the closure answered {called:?}"),
        Err(error) => error,
    };
    assert!(
        !matches!(error, Error::Task(TaskCause::CannotEnter)),
        "the third task entered the store and trapped there, got {error:?}"
    );
    assert!(
        seen.dropped.load(Ordering::Relaxed),
        "the host future was dropped by the time the entry returned the trap"
    );
    let spins_at_trap = seen.spins.load(Ordering::Relaxed);
    // The trap drops the future inside the turn that ran the trapping
    // task, where no poll of the store is lending it.
    let reach = seen.reach.lock().expect("reach").clone();
    assert_eq!(
        reach,
        Some(Err(
            Error::Scheduler(SchedulerCause::StoreNotInPoll).to_string()
        )),
        "the future's drop reached for the store and was refused, rather than deadlocking \
         or lending the store"
    );

    // A later driver meets no stale work: it runs no callback of
    // either task, however many turns it runs. The host future is
    // released and woken, so a store that still held it would poll it
    // to its end and run the first task's callback.
    seen.release.store(true, Ordering::Relaxed);
    if let Some(waker) = seen.waker.lock().expect("the waker").take() {
        waker.wake();
    }
    let polls = store
        .run_concurrent(async |_accessor| {
            let mut polls = 0;
            poll_fn(|context| {
                polls += 1;
                if polls == 16 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
            polls
        })
        .await
        .expect("a closure that does only host work runs in a poisoned store");
    assert_eq!(polls, 16, "the later driver ran its closure to the end");
    assert_eq!(
        seen.spins.load(Ordering::Relaxed),
        spins_at_trap,
        "the second task's guest code never ran again"
    );
    assert_eq!(
        seen.holds.load(Ordering::Relaxed),
        0,
        "the first task's callback never ran, as the host future it waited on was gone"
    );
}

/// A consumer of the host's own future that keeps the value it takes.
struct Keeps(Arc<Mutex<Option<u32>>>);

impl FutureConsumer<Drops> for Keeps {
    type Item = u32;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: &mut StoreContext<'_, Drops>,
        mut source: Source<'_, u32>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        let mut value = Vec::new();
        source.read(store, &mut value, 1)?;
        *self.0.lock().expect("the kept value") = value.pop();
        Poll::Ready(Ok(()))
    }
}

#[wcmp_macros::test]
async fn it_runs_a_pipe_of_the_hosts_own_that_starts_after_the_trap() {
    // The trap discards the work the store held at that moment. Host
    // work that starts afterwards touches no guest, and runs.
    let mut fixture = Fixture::new().await;
    fixture
        .call("unreachable", &[])
        .await
        .expect_err("the guest traps");

    let kept = Arc::new(Mutex::new(None));
    FutureReader::new(&mut fixture.store.as_context_mut(), async {
        Ok::<_, Error>(5u32)
    })
    .expect("a future the host writes")
    .pipe(&mut fixture.store.as_context_mut(), Keeps(kept.clone()))
    .expect("the host pipes its own future");

    fixture
        .store
        .run_concurrent(async |_accessor| {
            let mut polls = 0;
            poll_fn(|context| {
                polls += 1;
                if kept.lock().expect("the kept value").is_some() || polls == 64 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        })
        .await
        .expect("a closure that does only host work runs in a poisoned store");
    assert_eq!(
        *kept.lock().expect("the kept value"),
        Some(5),
        "the pipe handed the host's value to the host's consumer"
    );
}

/// A consumer of the host's own future that fails as it takes the
/// value.
struct Refuses;

impl FutureConsumer<Drops> for Refuses {
    type Item = u32;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: &mut StoreContext<'_, Drops>,
        _source: Source<'_, u32>,
        _finish: bool,
    ) -> Poll<Result<(), Error>> {
        Poll::Ready(Err(Error::Internal {
            message: "the host's consumer failed".to_owned(),
        }))
    }
}

#[wcmp_macros::test]
async fn it_leaves_the_store_usable_when_a_pipe_of_the_hosts_own_fails() {
    // The pipe touches no guest, so its failure is no trap. It ends the
    // entry whose turn met it, and the store runs guest code after it.
    let mut fixture = Fixture::new().await;
    FutureReader::new(&mut fixture.store.as_context_mut(), async {
        Ok::<_, Error>(5u32)
    })
    .expect("a future the host writes")
    .pipe(&mut fixture.store.as_context_mut(), Refuses)
    .expect("the host pipes its own future");

    let entry = fixture
        .store
        .run_concurrent(async |_accessor| {
            let mut polls = 0;
            poll_fn(|context| {
                polls += 1;
                if polls == 64 {
                    return Poll::Ready(());
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        })
        .await;
    let error = entry.expect_err("the failure ends the entry whose turn met it");
    assert!(
        error.to_string().contains("the host's consumer failed"),
        "the entry reports the consumer's failure, got {error:?}"
    );
    fixture
        .assert_usable("a failed pipe of the host's own")
        .await;
}
