//! Baseline tests for the concurrent call entry on `Func` and
//! `TypedFunc`.
//!
//! `call_concurrent` is the entry a host calls from inside a poll of
//! the store: from the closure of the store's `run_concurrent` entry,
//! or from the body of a host `async` function. It reaches the store
//! through the accessor those two are handed, creates the export's
//! task, and queues the start of that task's implicit thread behind
//! the entry gate of its instance, exactly as `Func::call` does. The
//! future it returns resolves when the task's result is set.
//!
//! The entry is not a driver, and `Func::call`, which is one, cannot
//! be entered from the closure at all: it takes the store by `&mut`
//! and the closure holds only the accessor. The task therefore
//! progresses only while something else runs turns — in practice
//! while the future is awaited inside the `run_concurrent` closure.
//!
//! Two overlapping calls observe the entry gate. A callback export's
//! task claims its instance as its start is queued, so a second call
//! into the same export waits at the gate and starts when the first
//! releases the instance, which a callback task does between events.
//! A synchronous export's task ignores the gate, so a call into one
//! runs at once even while a callback task of the same instance waits
//! in its event loop; a second such call is queued behind the first
//! and starts when the first returns, or inside the first's nested
//! turn when the first blocks and the two share an instance.
//!
//! The future is spawn-like. Dropping it cancels nothing: the task
//! stays in the store and runs in the next turn of any driver. And a
//! store that goes idle with the task unresolved leaves the
//! `run_concurrent` entry pending rather than failing it, so the
//! call's future simply never resolves — the closure around it can
//! still unblock the task with another call, and a host bounds the
//! whole entry from outside.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Accessor, Component, Engine, Func, HostCall, Instance, Linker, Result, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// One callback export called twice. Its core function logs the
/// argument, keeps it in the task's own context slot, and gives way;
/// its callback logs the argument again, one higher, returns ten
/// times the argument through `task.return`, and exits. Two calls
/// into it therefore log a line each before either logs its second,
/// which is what interleaving by events looks like from the host.
const CALLBACK_STEPS: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))
      (core func $log (canon lower (func $log)))
      (core func $task-return (canon task.return (result u32)))
      (core func $context-get (canon context.get i32 0))
      (core func $context-set (canon context.set i32 0))
      (core module $m
        (import "" "log" (func $log (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "context.get" (func $context-get (result i32)))
        (import "" "context.set" (func $context-set (param i32)))
        (func (export "step") (param i32) (result i32)
          (call $log (local.get 0))
          (call $context-set (local.get 0))
          (i32.const 1))
        (func (export "step-callback") (param i32 i32 i32) (result i32)
          (call $log (i32.add (call $context-get) (i32.const 1)))
          (call $task-return (i32.mul (call $context-get) (i32.const 10)))
          (i32.const 0)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "log" (func $log))
          (export "task.return" (func $task-return))
          (export "context.get" (func $context-get))
          (export "context.set" (func $context-set))))))
      (func (export "step") async (param "x" u32) (result u32)
        (canon lift (core func $i "step") async
          (callback (core func $i "step-callback")))))
    "#
);

/// Two callback exports and a synchronous one behind one core
/// instance. `awaited` returns its result and then waits on a
/// waitable set of its own that no turn ever fills, so its task sits
/// in its event loop for the rest of the test. `stuck` waits the same
/// way without returning a result at all, so its call never resolves.
/// `double` is the synchronous export a call runs against while
/// either of them waits.
const WAITING_CALLBACKS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (func $wait-word (result i32)
          (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2)))
        (func (export "awaited") (param i32) (result i32)
          (call $task-return (local.get 0))
          (call $wait-word))
        (func (export "stuck") (param i32) (result i32)
          (call $wait-word))
        (func (export "callback") (param i32 i32 i32) (result i32) unreachable)
        (func (export "double") (param i32) (result i32)
          local.get 0 i32.const 2 i32.mul))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))))))
      (func (export "awaited") async (param "x" u32) (result u32)
        (canon lift (core func $i "awaited") async (callback (core func $i "callback"))))
      (func (export "stuck") async (param "x" u32) (result u32)
        (canon lift (core func $i "stuck") async (callback (core func $i "callback"))))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

/// One synchronous export that logs its argument on the way in and
/// one higher on the way out, and returns ten times the argument. Two
/// calls into it log four entries in the order the two tasks ran.
const SYNC_STEPS: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))
      (core func $log (canon lower (func $log)))
      (core module $m
        (import "" "log" (func $log (param i32)))
        (func (export "step") (param i32) (result i32)
          (call $log (local.get 0))
          (call $log (i32.add (local.get 0) (i32.const 1)))
          (i32.mul (local.get 0) (i32.const 10))))
      (core instance $i (instantiate $m
        (with "" (instance (export "log" (func $log))))))
      (func (export "step") (param "x" u32) (result u32)
        (canon lift (core func $i "step"))))
    "#
);

/// Two synchronous exports of one component instance. `give-way`
/// logs, blocks on `thread.yield`, and logs again on the way out;
/// `step` is the plain pair of log entries. A synchronous task must
/// return before its instance may block, so the yield opens a nested
/// turn held to that instance's own ready work — which is where a
/// call into `step` queued behind it starts.
///
/// The second call enters the other export on purpose. In the
/// browser a lowered host function is one `FnMut` closure, and
/// wasm-bindgen refuses to invoke one recursively, so a guest that
/// re-entered `thread.yield` from inside the nested turn that
/// `thread.yield` opened would fail on that target and on no other.
const SYNC_YIELDS: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))
      (core func $log (canon lower (func $log)))
      (core func $yield (canon thread.yield))
      (core module $m
        (import "" "log" (func $log (param i32)))
        (import "" "thread.yield" (func $yield (result i32)))
        (func (export "give-way") (param i32) (result i32)
          (call $log (local.get 0))
          (drop (call $yield))
          (call $log (i32.add (local.get 0) (i32.const 1)))
          (i32.mul (local.get 0) (i32.const 10)))
        (func (export "step") (param i32) (result i32)
          (call $log (local.get 0))
          (call $log (i32.add (local.get 0) (i32.const 1)))
          (i32.mul (local.get 0) (i32.const 10))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "log" (func $log))
          (export "thread.yield" (func $yield))))))
      (func (export "give-way") (param "x" u32) (result u32)
        (canon lift (core func $i "give-way")))
      (func (export "step") (param "x" u32) (result u32)
        (canon lift (core func $i "step"))))
    "#
);

/// How many times a test polls an entry that is expected never to
/// resolve. A host bounds such an entry with a timeout; a test bounds
/// it with a fixed number of polls, which is the same bound in the
/// one unit both targets measure the same way.
const POLL_BUDGET: usize = 8;

/// What the guests logged, in the order they logged it.
type Log = Arc<Mutex<Vec<u32>>>;

/// What two overlapping calls produced, and what the store held the
/// moment both of their starts were queued.
struct TwoCalls {
    /// What the first call resolved to.
    first: Result<Box<[Val]>>,
    /// What the second call resolved to.
    second: Result<Box<[Val]>>,
    /// How many tasks waited at an entry gate with both starts
    /// queued and no turn yet run.
    at_gate: usize,
    /// How many items the store held at that same moment.
    queued: usize,
}

/// Instantiate `binary` in a fresh store with the host `log`
/// function registered, and hand back the list it appends to. A
/// component that imports no `log` is instantiated all the same: a
/// registration the component does not ask for is not linked.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance, Log) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let recorded = log.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_wrap(
        "log",
        move |_: HostCall<'_, ()>, (entry,): (u32,)| -> Result<()> {
            recorded.lock().expect("log").push(entry);
            Ok(())
        },
    );
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, log)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance.get_func(name).expect("the export is declared")
}

/// What the guests have logged so far.
fn entries(log: &Log) -> Vec<u32> {
    log.lock().expect("log").clone()
}

/// How many task records the store holds.
fn task_count(store: &Store<()>) -> usize {
    store
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .task_count()
}

/// Poll `future` once, as an executor would.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

/// Start two calls, each into the export it names with the argument
/// beside it, note what the store held the moment both starts were
/// queued and before any turn ran, and drive the two futures together
/// until both resolve.
///
/// The two futures are polled by hand rather than joined, because
/// what the test is about is what the store looks like between the
/// polls: the first poll of each future is what queues that call's
/// start, and the gate's answer is only meaningful once both are in.
async fn two_calls(
    accessor: &Accessor<()>,
    first_call: (&Func, u32),
    second_call: (&Func, u32),
) -> TwoCalls {
    let first_args = [Val::U32(first_call.1)];
    let second_args = [Val::U32(second_call.1)];
    let mut first = Box::pin(first_call.0.call_concurrent(accessor, &first_args));
    let mut second = Box::pin(second_call.0.call_concurrent(accessor, &second_args));
    let mut first_done: Option<Result<Box<[Val]>>> = None;
    let mut second_done: Option<Result<Box<[Val]>>> = None;
    let mut held: Option<(usize, usize)> = None;

    poll_fn(|context| {
        if first_done.is_none()
            && let Poll::Ready(value) = first.as_mut().poll(context)
        {
            first_done = Some(value);
        }
        if second_done.is_none()
            && let Poll::Ready(value) = second.as_mut().poll(context)
        {
            second_done = Some(value);
        }
        if held.is_none() {
            held = Some(
                accessor
                    .with(|store| {
                        (
                            store.scheduler().waiting_at_gate(),
                            store.scheduler().queued_items(),
                        )
                    })
                    .expect("reach the store"),
            );
        }
        if first_done.is_some() && second_done.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;

    let (at_gate, queued) = held.expect("the store was read on the first poll");
    TwoCalls {
        first: first_done.expect("the first call resolved"),
        second: second_done.expect("the second call resolved"),
        at_gate,
        queued,
    }
}

#[wcmp_macros::test]
async fn it_interleaves_two_calls_into_one_callback_export_by_events() {
    let (mut store, instance, log) = instantiate(CALLBACK_STEPS).await;
    let step = func(&instance, "step");

    let calls = store
        .run_concurrent(async |accessor| two_calls(accessor, (&step, 10), (&step, 20)).await)
        .await
        .expect("run the closure");

    assert_eq!(
        calls.at_gate, 1,
        "the first task claimed the instance as its start was queued, so the \
         second call waits at the entry gate"
    );
    assert_eq!(
        calls.queued, 2,
        "one start is ready and the other is held at the gate"
    );
    assert_eq!(
        calls.first.expect("the first call resolves").as_ref(),
        [Val::U32(100)],
        "the first call resolves with what its task returned"
    );
    assert_eq!(
        calls.second.expect("the second call resolves").as_ref(),
        [Val::U32(200)],
        "and so does the second"
    );
    assert_eq!(
        entries(&log),
        vec![10, 20, 11, 21],
        "the second task started when the first gave way, and the two then \
         interleaved by events: both core functions ran before either callback"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_call_into_a_synchronous_export_while_a_callback_task_waits() {
    let (mut store, instance, _log) = instantiate(WAITING_CALLBACKS).await;
    let awaited = func(&instance, "awaited");
    let double = func(&instance, "double");

    let (returned, held_before, doubled, held_after) = store
        .run_concurrent(async |accessor| {
            let awaited_args = [Val::U32(5)];
            let returned = awaited.call_concurrent(accessor, &awaited_args).await;
            // The task returned its result and then waited on a set
            // no turn ever fills, so its callback item is held in the
            // store and its task is in its event loop.
            let held_before = accessor
                .with(|store| store.scheduler().held_callbacks())
                .expect("reach the store");
            let double_args = [Val::U32(21)];
            let doubled = double.call_concurrent(accessor, &double_args).await;
            let held_after = accessor
                .with(|store| store.scheduler().held_callbacks())
                .expect("reach the store");
            (returned, held_before, doubled, held_after)
        })
        .await
        .expect("run the closure");

    assert_eq!(
        returned
            .expect("the callback export returns its result")
            .as_ref(),
        [Val::U32(5)],
        "the callback task returned through `task.return` before it waited"
    );
    assert_eq!(
        held_before, 1,
        "the callback task is waiting in its event loop"
    );
    assert_eq!(
        doubled.expect("the synchronous export is called").as_ref(),
        [Val::U32(42)],
        "the synchronous export runs at once: its task ignores the entry gate, \
         and the waiting task released the instance between events"
    );
    assert_eq!(
        held_after, 1,
        "and the callback task is still waiting, so the synchronous call ran \
         beside it rather than after it"
    );
}

#[wcmp_macros::test]
async fn it_queues_a_second_call_into_a_synchronous_export_behind_the_first() {
    let (mut store, instance, log) = instantiate(SYNC_STEPS).await;
    let step = func(&instance, "step");

    let calls = store
        .run_concurrent(async |accessor| two_calls(accessor, (&step, 1), (&step, 10)).await)
        .await
        .expect("run the closure");

    assert_eq!(
        calls.at_gate, 0,
        "a synchronous export's task ignores the entry gate"
    );
    assert_eq!(
        calls.queued, 2,
        "both starts are queued and no turn has run either"
    );
    assert_eq!(
        calls.first.expect("the first call resolves").as_ref(),
        [Val::U32(10)],
        "the first call resolves with what its task returned"
    );
    assert_eq!(
        calls.second.expect("the second call resolves").as_ref(),
        [Val::U32(100)],
        "and so does the second"
    );
    assert_eq!(
        entries(&log),
        vec![1, 2, 10, 11],
        "the second task started when the first returned"
    );
}

#[wcmp_macros::test]
async fn it_starts_a_queued_synchronous_call_inside_the_first_tasks_nested_turn() {
    let (mut store, instance, log) = instantiate(SYNC_YIELDS).await;
    let give_way = func(&instance, "give-way");
    let step = func(&instance, "step");

    let calls = store
        .run_concurrent(async |accessor| two_calls(accessor, (&give_way, 1), (&step, 10)).await)
        .await
        .expect("run the closure");

    assert_eq!(
        calls.first.expect("the first call resolves").as_ref(),
        [Val::U32(10)],
        "the first call resolves with what its task returned"
    );
    assert_eq!(
        calls.second.expect("the second call resolves").as_ref(),
        [Val::U32(100)],
        "and so does the second"
    );
    assert_eq!(
        entries(&log),
        vec![1, 10, 11, 2],
        "the first task blocked, and the nested turn its block opened ran the \
         ready work of its own instance — the second call's start — which \
         therefore ran to its return inside the first's block"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_task_of_a_dropped_call_in_the_store() {
    let (mut store, instance, log) = instantiate(SYNC_STEPS).await;
    let step = func(&instance, "step");

    let (pending, before, second) = store
        .run_concurrent(async |accessor| {
            let dropped_args = [Val::U32(1)];
            let mut dropped = Box::pin(step.call_concurrent(accessor, &dropped_args));
            // One poll queues the task's start and nothing else: a
            // turn is what runs it, and the closure's future is
            // polled between turns.
            let pending =
                poll_fn(|context| Poll::Ready(dropped.as_mut().poll(context).is_pending())).await;
            drop(dropped);
            let before = entries(&log);

            // Another call is what drives the store from here, and it
            // runs the abandoned task's start in the same turn.
            let second_args = [Val::U32(10)];
            let second = step.call_concurrent(accessor, &second_args).await;
            (pending, before, second)
        })
        .await
        .expect("run the closure");

    assert!(
        pending,
        "the call had not resolved when its future was dropped"
    );
    assert!(
        before.is_empty(),
        "and its task had not run either, so the drop cancelled something \
         that was still queued"
    );
    assert_eq!(
        second.expect("the second call resolves").as_ref(),
        [Val::U32(100)],
        "the call that came after the drop resolves"
    );
    assert_eq!(
        entries(&log),
        vec![1, 2, 10, 11],
        "dropping the future cancelled nothing: the task stayed in the store \
         and ran in the next turn of another driver"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_entry_pending_when_the_store_goes_idle_with_the_task_unresolved() {
    let (mut store, instance, _log) = instantiate(WAITING_CALLBACKS).await;
    let stuck = func(&instance, "stuck");
    let args = [Val::U32(5)];

    let mut entry = Box::pin(
        store.run_concurrent(async |accessor| stuck.call_concurrent(accessor, &args).await),
    );

    for poll in 0..POLL_BUDGET {
        assert!(
            poll_once(&mut entry, Waker::noop()).is_pending(),
            "poll {poll} of the entry: the task waits on a set no turn fills, \
             so the store goes idle with it unresolved and the entry parks \
             rather than failing with the deadlock cause"
        );
    }
    drop(entry);

    assert_eq!(
        task_count(&store),
        1,
        "the task is still in the store, unresolved, for a later entry to \
         unblock"
    );
}

#[wcmp_macros::test]
async fn it_returns_the_typed_result_through_the_typed_entry() {
    let (mut store, instance, log) = instantiate(SYNC_STEPS).await;
    let concurrent = func(&instance, "step")
        .typed::<(u32,), u32>()
        .expect("the typed handle is acquired as it is for a direct call");

    let through_the_accessor = store
        .run_concurrent(async |accessor| concurrent.call_concurrent(accessor, (3,)).await)
        .await
        .expect("run the closure")
        .expect("call the export");

    let directly = func(&instance, "step")
        .typed::<(u32,), u32>()
        .expect("typed handle")
        .call(&mut store, (3,))
        .await
        .expect("call the export");

    assert_eq!(
        through_the_accessor, 30,
        "the typed entry returns the native Rust value the export produced"
    );
    assert_eq!(
        through_the_accessor, directly,
        "and it is the value the same export gives the typed direct call, \
         reached with the same host code"
    );
    assert_eq!(
        entries(&log),
        vec![3, 4, 3, 4],
        "both calls ran the export's body"
    );
}
