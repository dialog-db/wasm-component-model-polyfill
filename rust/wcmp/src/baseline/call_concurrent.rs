// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
//!
//! The future leaves its waker behind, and the turn that resolves the
//! task wakes it. That is what lets a host hand several of these
//! futures to a combinator which re-polls a child only after that
//! child's waker fires, the shape of `FuturesUnordered` and its kin,
//! rather than polling each of them by hand after every turn. A trap
//! of the task does not come back through the future: it ends the
//! `run_concurrent` entry that was polling, and the closure goes with
//! every future inside it.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Wake;

use crate::internal::ResourceTypeIdInternal;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    AbiCause, Accessor, Component, Engine, Error, Func, HostCall, HostResource, Instance,
    InterfaceIdentifier, Linker, ResourceTypeId, Result, Store, TaskCause, Val,
};
use wcmp_macros::component;

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
/// call into either export queued behind it starts.
///
/// A call into `step` runs no host function the block is inside. A
/// call into `give-way` reaches `thread.yield` while the first
/// call's `thread.yield` is still on the stack, which both backends
/// serve. The two tests below take one case each.
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

/// One synchronous export over a borrow of an imported resource,
/// which it keeps: the guest owes the borrow back at the end of the
/// call and never drops it. The task resolves — a function with no
/// result resolves with none — and the call fails afterwards, on the
/// borrows the guest still owed as its thread ended.
const BORROW_HOLDER: &[u8] = component!(
    r#"
    (component
      (import "pdd020-tests:host/things@0.1.0" (instance $i
        (export "thing" (type $thing (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core module $m
        (func (export "hold") (param i32)))
      (core instance $c (instantiate $m))
      (func (export "hold") (param "h" (borrow $thing))
        (canon lift (core func $c "hold"))))
    "#
);

/// One callback export that lends a host handle and keeps running.
///
/// Its core function calls the host's `lend`, which is where the
/// host records a handle of its own as lent for this call, then
/// `task.return`s and gives way. Its callback waits on a fresh set
/// no turn ever fills, so the task stays in the store after the
/// call's future has resolved. That is the gap the host lend's rule
/// is read in: the lend ends with the resolution, not with the task.
const LENDS_AND_KEEPS_RUNNING: &[u8] = component!(
    r#"
    (component
      (import "lend" (func $lend))
      (core func $lend (canon lower (func $lend)))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core module $m
        (import "" "lend" (func $lend))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (func (export "answer") (param i32) (result i32)
          (call $lend)
          (call $task-return (i32.mul (local.get 0) (i32.const 2)))
          (i32.const 1))
        (func (export "callback") (param i32 i32 i32) (result i32)
          (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "lend" (func $lend))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))))))
      (func (export "answer") async (param "x" u32) (result u32)
        (canon lift (core func $i "answer") async (callback (core func $i "callback")))))
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
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let recorded = log.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "log",
            move |_: HostCall<'_, ()>, (entry,): (u32,)| -> Result<()> {
                recorded.lock().expect("log").push(entry);
                Ok(())
            },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, log)
}

/// Instantiate [`BORROW_HOLDER`] in a fresh store, with the `thing`
/// resource registered under the label the component imports it by,
/// and hand back the identity a host handle is minted against.
async fn instantiate_borrow_holder() -> (Store<()>, Instance, ResourceTypeId) {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, BORROW_HOLDER)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    let interface: InterfaceIdentifier = "pdd020-tests:host/things@0.1.0"
        .parse()
        .expect("identifier");
    let type_id = linker
        .instance(&interface)
        .resource_with(
            "thing",
            HostResource::new(|_: &mut (), _: u32| -> Result<()> { Ok(()) }),
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance, type_id)
}

/// One child of the combinator below: the call's future, the gate
/// that says whether the call has been woken since it was last
/// polled, and what the call resolved to.
struct Gated<F: Future> {
    future: Pin<Box<F>>,
    gate: Arc<Gate>,
    done: Option<F::Output>,
}

/// The waker one child of the combinator is polled with. It records
/// that the child was woken and passes the wake on to whoever is
/// polling the combinator, which is what every combinator of the
/// `FuturesUnordered` shape does with a child's waker.
#[derive(Default)]
struct Gate {
    woken: AtomicBool,
    parent: Mutex<Option<Waker>>,
}

impl Gate {
    /// A gate for a child that has yet to be polled once, which
    /// counts as woken: a combinator polls each child it is given
    /// before it waits on any of them.
    fn unpolled() -> Arc<Self> {
        Arc::new(Self {
            woken: AtomicBool::new(true),
            parent: Mutex::new(None),
        })
    }

    /// Remember the waker of whoever is polling the combinator, so
    /// that a wake the child receives between two of those polls
    /// reaches them.
    fn watch(&self, parent: &Waker) {
        *self.parent.lock().expect("parent waker") = Some(parent.clone());
    }

    /// Whether the child was woken since it was last polled. Taking
    /// the wake clears it: a child that is polled and returns pending
    /// again waits for the next one.
    fn take_wake(&self) -> bool {
        self.woken.swap(false, Ordering::SeqCst)
    }
}

impl Wake for Gate {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
        let parent = self.parent.lock().expect("parent waker").clone();
        if let Some(parent) = parent {
            parent.wake();
        }
    }
}

/// Poll `calls` together through a combinator that re-polls a child
/// only after that child's waker has fired, and hand back what each
/// of them resolved to.
///
/// This is the shape of every host combinator that joins futures —
/// `FuturesUnordered`, `select_all`, a `JoinSet`. A call that
/// returned pending without leaving its waker behind is never polled
/// again here, however many turns the driver around the combinator
/// runs, so this is what a call that does not wake its caller looks
/// like from a host.
///
/// The combinator gives up when a call is still pending and the store
/// can no longer carry anything forward: nothing queued, nothing
/// ready, and no host task. That is the shape a lost wake leaves
/// behind — the results are in their slots, the store is empty, and
/// nobody reads them — and giving up there is what turns it into a
/// failed assertion rather than a test that never returns.
async fn join_gated<F: Future>(accessor: &Accessor<()>, calls: Vec<F>) -> Vec<Option<F::Output>> {
    let mut children: Vec<Gated<F>> = calls
        .into_iter()
        .map(|call| Gated {
            future: Box::pin(call),
            gate: Gate::unpolled(),
            done: None,
        })
        .collect();

    poll_fn(|context| {
        let mut pending = false;
        for child in &mut children {
            if child.done.is_some() {
                continue;
            }
            child.gate.watch(context.waker());
            if !child.gate.take_wake() {
                pending = true;
                continue;
            }
            let waker = Waker::from(child.gate.clone());
            let mut gated = Context::from_waker(&waker);
            match child.future.as_mut().poll(&mut gated) {
                Poll::Ready(value) => child.done = Some(value),
                Poll::Pending => pending = true,
            }
        }
        if pending && !store_is_spent(accessor) {
            return Poll::Pending;
        }
        Poll::Ready(())
    })
    .await;

    children.into_iter().map(|child| child.done).collect()
}

/// Whether the store holds nothing that could carry a pending call
/// forward.
fn store_is_spent(accessor: &Accessor<()>) -> bool {
    accessor
        .with(|store| {
            !store.internal().has_pending_work() && store.internal().scheduler().queued_items() == 0
        })
        .expect("reach the store")
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
        .internal_ref()
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
                            store.internal().scheduler().waiting_at_gate(),
                            store.internal().scheduler().queued_items(),
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
                .with(|store| store.internal().scheduler().held_callbacks())
                .expect("reach the store");
            let double_args = [Val::U32(21)];
            let doubled = double.call_concurrent(accessor, &double_args).await;
            let held_after = accessor
                .with(|store| store.internal().scheduler().held_callbacks())
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
async fn it_runs_a_queued_call_that_re_enters_the_import_the_block_is_inside() {
    let (mut store, instance, log) = instantiate(SYNC_YIELDS).await;
    let give_way = func(&instance, "give-way");

    let calls = store
        .run_concurrent(async |accessor| two_calls(accessor, (&give_way, 1), (&give_way, 10)).await)
        .await
        .expect("run the closure");

    assert_eq!(
        calls.first.expect("the first call resolves").as_ref(),
        [Val::U32(10)],
        "the first call blocks, its nested turn runs the second call's start, \
         and it returns whatever became of that"
    );
    assert_eq!(
        calls.second.expect("the second call resolves").as_ref(),
        [Val::U32(100)],
        "both backends call a host function already on the stack, so the \
         second task's own yield returns and the task runs to its end"
    );
    assert_eq!(
        entries(&log),
        vec![1, 10, 11, 2],
        "the second task ran to its return inside the first task's block"
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

#[wcmp_macros::test]
async fn it_resolves_every_call_a_waker_gated_combinator_holds() {
    let (mut store, instance, log) = instantiate(CALLBACK_STEPS).await;
    let step = func(&instance, "step");
    let first_args = [Val::U32(10)];
    let second_args = [Val::U32(20)];
    let third_args = [Val::U32(30)];

    let resolved = store
        .run_concurrent(async |accessor| {
            join_gated(
                accessor,
                vec![
                    step.call_concurrent(accessor, &first_args),
                    step.call_concurrent(accessor, &second_args),
                    step.call_concurrent(accessor, &third_args),
                ],
            )
            .await
        })
        .await
        .expect("run the closure");

    let results: Vec<Option<Box<[Val]>>> = resolved
        .into_iter()
        .map(|call| call.map(|value| value.expect("the call resolves")))
        .collect();
    assert_eq!(
        results,
        vec![
            Some(vec![Val::U32(100)].into_boxed_slice()),
            Some(vec![Val::U32(200)].into_boxed_slice()),
            Some(vec![Val::U32(300)].into_boxed_slice()),
        ],
        "every call was polled again after the turn that resolved its task, \
         because resolving the task woke the caller; a call that left no \
         waker behind would still be pending here, with its result in a slot \
         nobody reads"
    );
    assert_eq!(
        entries(&log),
        vec![10, 20, 30, 11, 21, 31],
        "the three tasks passed the gate in order and interleaved by events, \
         as two of them do when the host polls them by hand"
    );
}

#[wcmp_macros::test]
async fn it_ends_the_entry_with_the_borrows_a_concurrent_calls_guest_still_owes() {
    let (mut store, instance, type_id) = instantiate_borrow_holder().await;
    let handle = store.resource_new(type_id, 5).expect("mint an own handle");
    let hold = func(&instance, "hold");
    let args = [Val::Borrow(handle)];

    let failure = match store
        .run_concurrent(async |accessor| {
            join_gated(accessor, vec![hold.call_concurrent(accessor, &args)]).await
        })
        .await
    {
        Ok(_) => panic!("the guest kept the borrow, so the entry must fail"),
        Err(failure) => failure,
    };
    assert!(
        matches!(&failure, Error::Abi(abi) if matches!(
            abi.cause,
            AbiCause::OutstandingBorrows { count: 1 }
        )),
        "the entry that was polling fails with the borrow the guest still \
         owed, which the task raised after it had already resolved: {failure}"
    );

    let refused = hold
        .call(&mut store, &args)
        .await
        .expect_err("the trap poisoned the store");
    assert!(
        matches!(refused, Error::Task(TaskCause::CannotEnter)),
        "the next driver fails with the cannot-enter cause, got {refused:?}"
    );
}

#[wcmp_macros::test]
async fn it_gives_a_host_lend_back_when_the_awaited_future_resolves() {
    // A handle the host lends for its call into a guest export goes
    // on the export's task, and comes back when that task resolves,
    // which for `call_concurrent` is when the awaited future does.
    // The export here resolves and keeps running, so the task is
    // still in the store when the future hands the result back: the
    // drop below therefore says that the resolution and not the
    // task's exit is what ended the lend.
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let component = Component::new(&engine, LENDS_AND_KEEPS_RUNNING)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let tables = store.internal().tables().clone();
    let ty = ResourceTypeId::fresh();
    // What the host lent, and whether the lend stood at the moment
    // it was made, which the `lend` import records from inside the
    // call.
    let lent = Arc::new(Mutex::new(None));
    let stood = Arc::new(AtomicBool::new(false));
    let recorded = lent.clone();
    let witness = stood.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("lend", move |_: HostCall<'_, ()>, (): ()| -> Result<()> {
            let mut guard = tables.lock().expect("handle tables");
            // The export's task is the outermost scope on the stack:
            // the host call the guest is inside pushed a subtask of
            // its own above it.
            let scope = guard
                .tasks
                .scopes()
                .first()
                .copied()
                .expect("the export's task is on the stack");
            let table = guard.host_table(ty);
            let index = guard.insert_own(table, ty, false, 11);
            guard
                .lend_to(Some(scope), table, index)
                .expect("the host lends a handle of its own for the call");
            witness.store(
                guard.remove_own(table, index, ty, false).is_err(),
                Ordering::SeqCst,
            );
            *recorded.lock().expect("the lend") = Some((table, index));
            Ok(())
        })
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let answer = func(&instance, "answer");

    let returned = store
        .run_concurrent(async |accessor| answer.call_concurrent(accessor, &[Val::U32(21)]).await)
        .await
        .expect("run the closure")
        .expect("the callback export returns its result");
    assert_eq!(
        returned.as_ref(),
        [Val::U32(42)],
        "the task returned through `task.return`, which resolved the future"
    );
    assert!(
        stood.load(Ordering::SeqCst),
        "the handle was lent while the call was in flight"
    );

    let (table, index) = lent
        .lock()
        .expect("the lend")
        .expect("the guest called the host's `lend`");
    let mut guard = store.internal().tables().lock().expect("handle tables");
    assert_eq!(
        guard.tasks.task_count(),
        1,
        "the callback task kept running past its `task.return`"
    );
    assert_eq!(
        guard.remove_own(table, index, ty, false),
        Ok(11),
        "the resolution of the awaited future gave the host its handle back"
    );
}
