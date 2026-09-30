//! Host suspension.

use core::future::{Future, poll_fn};
use core::pin::pin;
use core::task::{Poll, Waker};
use std::sync::{Arc, Mutex, PoisonError};

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Caller, Capability, Engine, Error, Func, FuncType, Instance, ResumableCall, Store,
    SuspendedCall, TrapKind, Val, ValType,
};

use crate::support;

/// Where the engine does not declare `host_suspension`, a suspending host
/// function and a resumable call are each
/// [`Error::Unsupported`] with `host_suspension`.
pub async fn it_refuses_host_suspension_where_it_is_not_declared(engine: &Engine) {
    if support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, ());

    let suspending =
        Func::new_suspending(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
            Ok(Poll::Pending)
        });
    assert!(
        matches!(
            suspending,
            Err(Error::Unsupported(Capability::HostSuspension))
        ),
        "{suspending:?}"
    );

    let instance = support::instance(
        &mut store,
        wasm!(r#"(module (func (export "answer") (result i32) i32.const 42))"#),
        &[],
    )
    .await;
    let answer = support::func(&mut store, instance, "answer");
    let resumable = answer
        .call_resumable(&mut store, &[], &mut [Val::I32(0)])
        .await;
    assert!(
        matches!(
            resumable,
            Err(Error::Unsupported(Capability::HostSuspension))
        ),
        "{resumable:?}"
    );
}

/// What the host functions of a test of suspension record, outside the
/// store, so that the record outlives the store: each event in order, and
/// the waker of the test that waits for the next one.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<(Vec<Event>, Option<Waker>)>>);

/// What a host function of a test of suspension saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// The suspending host function was called with this argument, and
    /// answered "not yet".
    Waits(i32),
    /// The host function `note` was called with this argument.
    Notes(i32),
}

impl Log {
    /// Records `event`, and wakes the test that waits for it.
    fn push(&self, event: Event) {
        let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        log.0.push(event);
        if let Some(waker) = log.1.take() {
            waker.wake();
        }
    }

    /// Every event so far.
    fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .clone()
    }

    /// Waits until the log holds `count` events.
    async fn reaches(&self, count: usize) {
        poll_fn(|context| {
            let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if log.0.len() >= count {
                Poll::Ready(())
            } else {
                log.1 = Some(context.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }
}

/// The suspending host function `wait` of type `[i32] -> [i32]`, which
/// records its argument and answers "not yet", and the host function `note`
/// of type `[i32] -> []`, which records its argument, both in `log`.
fn host_functions<T: 'static>(store: &mut Store<T>, log: &Log) -> (Func, Func) {
    let waits = log.clone();
    let wait = Func::new_suspending(
        &mut *store,
        FuncType::new([ValType::I32], [ValType::I32]),
        move |_, params, _| {
            waits.push(Event::Waits(argument(params)?));
            Ok(Poll::Pending)
        },
    )
    .expect("the store makes a suspending host function");
    let notes = log.clone();
    let note = Func::new(
        &mut *store,
        FuncType::new([ValType::I32], []),
        move |_, params, _| {
            notes.push(Event::Notes(argument(params)?));
            Ok(())
        },
    )
    .expect("the store makes a host function");
    (wait, note)
}

/// The one `i32` of `values`.
fn argument(values: &[Val]) -> anyhow::Result<i32> {
    match values {
        [Val::I32(value)] => Ok(*value),
        _ => anyhow::bail!("{values:?} is not one i32"),
    }
}

/// The handle of a call that ended at `outcome`, which must wait.
fn waiting(outcome: Result<ResumableCall, Error>) -> SuspendedCall {
    match outcome {
        Ok(ResumableCall::Suspended(handle)) => handle,
        other => panic!("the call waits: {other:?}"),
    }
}

/// The results of a call that ended at `outcome` with `results`, which
/// must have finished.
fn finished(outcome: Result<ResumableCall, Error>, results: &[Val]) -> Vec<Option<i32>> {
    match outcome {
        Ok(ResumableCall::Finished) => results.iter().map(Val::i32).collect(),
        other => panic!("the call finishes: {other:?}"),
    }
}

/// What the store of the test of a host frame holds: the guest function
/// that the host function `through` calls back into.
#[derive(Default)]
struct Through {
    inner: Option<Func>,
}

/// Three resumable calls in one store each suspend in a suspending host
/// function, and wait at once. A fourth call reaches the suspending host
/// function through a host function that calls back into the guest, so a
/// frame of the host lies between the start of the call and the
/// suspension, and the call traps with [`TrapKind::Host`] and a message
/// that says the call cannot suspend. The three that wait are resumed
/// third, first, second, and each finishes with its own results.
pub async fn it_resumes_calls_that_wait_at_once_in_any_order(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, Through::default());
    let log = Log::default();
    let (wait, note) = host_functions(&mut store, &log);
    let through = Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32, ValType::I32]),
        |mut caller: Caller<'_, Through>, params, results| {
            let inner = caller
                .data()
                .inner
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            inner.call(&mut caller, params, results)?;
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (import "host" "wait" (func $wait (param i32) (result i32)))
              (import "host" "note" (func $note (param i32)))
              (import "host" "through" (func $through (param i32) (result i32 i32)))
              ;; The argument, and the argument times 100 plus the result of
              ;; `wait`.
              (func $run (export "run") (param i32) (result i32 i32)
                (local $resumed i32)
                local.get 0
                call $wait
                local.set $resumed
                local.get $resumed
                call $note
                local.get 0
                local.get 0
                i32.const 100
                i32.mul
                local.get $resumed
                i32.add)
              (func (export "outer") (param i32) (result i32 i32)
                local.get 0
                call $through))
            "#
        ),
        &[wait.into(), note.into(), through.into()],
    )
    .await;
    let run = support::func(&mut store, instance, "run");
    store.data_mut().inner = Some(run);
    let outer = support::func(&mut store, instance, "outer");

    let mut handles = Vec::new();
    for argument in 1..=3 {
        let mut results = [Val::I32(0), Val::I32(0)];
        let outcome = run
            .call_resumable(&mut store, &[Val::I32(argument)], &mut results)
            .await;
        handles.push(Some(waiting(outcome)));
    }
    assert_eq!(
        log.events(),
        [Event::Waits(1), Event::Waits(2), Event::Waits(3)]
    );

    let mut results = [Val::I32(0), Val::I32(0)];
    let fourth = outer
        .call_resumable(&mut store, &[Val::I32(4)], &mut results)
        .await;
    assert!(
        fourth.as_ref().err().is_some_and(cannot_suspend),
        "a suspension with a host frame below it traps: {fourth:?}"
    );
    assert_eq!(log.events().last(), Some(&Event::Waits(4)));

    for (place, import_result) in [(2, 30), (0, 10), (1, 20)] {
        let handle = handles[place].take().expect("each call resumes once");
        let mut results = [Val::I32(0), Val::I32(0)];
        let outcome = handle
            .resume(&mut store, &[Val::I32(import_result)], &mut results)
            .await;
        let argument = place as i32 + 1;
        assert_eq!(
            finished(outcome, &results),
            [Some(argument), Some(argument * 100 + import_result)],
            "the call of {argument} finishes with its own results"
        );
    }
    assert_eq!(
        log.events()[4..],
        [Event::Notes(30), Event::Notes(10), Event::Notes(20)]
    );
}

/// The guest of the tests of a resumption in flight: `run` waits, notes
/// what it was resumed with, and waits again with one more.
const TWICE: &[u8] = wasm!(
    r#"
    (module
      (import "host" "wait" (func $wait (param i32) (result i32)))
      (import "host" "note" (func $note (param i32)))
      (func (export "run") (param i32) (result i32)
        local.get 0
        call $wait
        call $note
        i32.const 8
        call $wait))
    "#
);

/// A store drops while two calls wait in it and a resumption of a third is
/// under way. Nothing panics. The resumption runs to its next suspension,
/// with its host functions, and the calls that waited never run again: one
/// drops before the store and one after it, and the one after it does not
/// resume in another store.
pub async fn it_runs_a_resumption_in_flight_to_its_next_stop_when_the_store_drops(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, ());
    let log = Log::default();
    let (wait, note) = host_functions(&mut store, &log);
    let instance = support::instance(&mut store, TWICE, &[wait.into(), note.into()]).await;
    let run = support::func(&mut store, instance, "run");

    let mut handles = Vec::new();
    for argument in 1..=3 {
        let mut results = [Val::I32(0)];
        let outcome = run
            .call_resumable(&mut store, &[Val::I32(argument)], &mut results)
            .await;
        handles.push(waiting(outcome));
    }
    let resumed = handles.remove(0);
    let before = handles.remove(0);
    let after = handles.remove(0);

    let mut results = [Val::I32(0)];
    {
        let mut resumption = pin!(resumed.resume(&mut store, &[Val::I32(7)], &mut results));
        // One poll starts the resumption. A backend that resumes on a
        // microtask leaves it under way.
        let first = poll_fn(|context| Poll::Ready(resumption.as_mut().poll(context))).await;
        if let Poll::Ready(outcome) = first {
            let handle = waiting(outcome);
            drop(handle);
        }
    }
    drop(before);
    drop(store);
    let mut other = support::store(engine, ());
    let stray = after
        .resume(&mut other, &[Val::I32(9)], &mut [Val::I32(0)])
        .await;
    assert!(
        matches!(stray, Err(Error::WrongStore)),
        "a call does not resume in another store: {stray:?}"
    );

    log.reaches(5).await;
    assert_eq!(
        log.events(),
        [
            Event::Waits(1),
            Event::Waits(2),
            Event::Waits(3),
            Event::Notes(7),
            Event::Waits(8),
        ]
    );
}

/// The future of a resumption drops before the resumption stops, and the
/// host uses the store. The resumption reaches the store no more: its host
/// functions never run. The store serves the next resumption as before:
/// the call it resumes suspends again, and finishes at its last
/// resumption.
pub async fn it_gives_the_store_back_when_the_future_of_a_resumption_drops(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, 0u32);
    let log = Log::default();
    let (wait, note) = host_functions(&mut store, &log);
    let instance = support::instance(&mut store, TWICE, &[wait.into(), note.into()]).await;
    let run = support::func(&mut store, instance, "run");

    let mut handles = Vec::new();
    for argument in 1..=2 {
        let mut results = [Val::I32(0)];
        let outcome = run
            .call_resumable(&mut store, &[Val::I32(argument)], &mut results)
            .await;
        handles.push(waiting(outcome));
    }
    let dropped = handles.remove(0);
    let kept = handles.remove(0);

    let mut results = [Val::I32(0)];
    let under_way = {
        let mut resumption = pin!(dropped.resume(&mut store, &[Val::I32(7)], &mut results));
        let first = poll_fn(|context| Poll::Ready(resumption.as_mut().poll(context))).await;
        first.is_pending()
    };
    *store.data_mut() += 1;

    // The resumption of the first call, where it was under way, was
    // started before this one, so it has reached its stack by the time
    // this one stops.
    let mut results = [Val::I32(0)];
    let outcome = kept.resume(&mut store, &[Val::I32(5)], &mut results).await;
    let again = waiting(outcome);
    let expected = if under_way {
        vec![
            Event::Waits(1),
            Event::Waits(2),
            Event::Notes(5),
            Event::Waits(8),
        ]
    } else {
        vec![
            Event::Waits(1),
            Event::Waits(2),
            Event::Notes(7),
            Event::Waits(8),
            Event::Notes(5),
            Event::Waits(8),
        ]
    };
    assert_eq!(log.events(), expected);
    assert_eq!(*store.data(), 1);

    // The call that suspended again finishes at its last resumption, with
    // the result of the second suspension.
    let mut results = [Val::I32(0)];
    let outcome = again.resume(&mut store, &[Val::I32(9)], &mut results).await;
    assert_eq!(finished(outcome, &results), [Some(9)]);
    assert_eq!(log.events(), expected, "no host function ran again");
}

/// A call resumed in two steps, with a resumption and a wait for its stop,
/// keeps its place where the future of a wait drops. One wait drops before
/// it is polled, and one after its first poll, where the call is still under
/// way. The host uses the store in between, and the call reaches the store
/// no more meanwhile: its host functions do not run. The next wait takes
/// the call up, and it runs on to its next suspension, host functions
/// included. The handle of that suspension finishes the call, and the
/// spent resumption answers no second stop.
pub async fn it_takes_up_a_resumption_whose_wait_dropped(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let mut store = support::store(engine, 0u32);
    let log = Log::default();
    let (wait, note) = host_functions(&mut store, &log);
    let instance = support::instance(&mut store, TWICE, &[wait.into(), note.into()]).await;
    let run = support::func(&mut store, instance, "run");

    let mut results = [Val::I32(0)];
    let outcome = run
        .call_resumable(&mut store, &[Val::I32(1)], &mut results)
        .await;
    let mut resumption = waiting(outcome)
        .start_resume(&mut store, &[Val::I32(7)])
        .expect("the call resumes");

    let mut results = [Val::I32(0)];
    drop(resumption.stop(&mut store, &mut results));
    let first = {
        let mut stop = pin!(resumption.stop(&mut store, &mut results));
        poll_fn(|context| Poll::Ready(stop.as_mut().poll(context))).await
    };
    let outcome = match first {
        Poll::Ready(outcome) => outcome,
        // A backend that resumes on a microtask leaves the call under way.
        Poll::Pending => {
            *store.data_mut() += 1;
            let_others_run(8).await;
            assert_eq!(
                log.events(),
                [Event::Waits(1)],
                "the call reaches the store no more while nothing waits for it"
            );
            resumption.stop(&mut store, &mut results).await
        }
    };
    let again = waiting(outcome);
    assert_eq!(
        log.events(),
        [Event::Waits(1), Event::Notes(7), Event::Waits(8)]
    );

    let mut results = [Val::I32(0)];
    let outcome = again.resume(&mut store, &[Val::I32(9)], &mut results).await;
    assert_eq!(finished(outcome, &results), [Some(9)]);
    let spent = resumption.stop(&mut store, &mut results).await;
    assert!(
        matches!(spent, Err(Error::Backend { .. })),
        "a resumption answers one stop: {spent:?}"
    );
}

/// Lets the host's executor run other work `times` times, the browser's
/// microtasks included, before it polls the caller again.
async fn let_others_run(times: usize) {
    for _ in 0..times {
        let mut yielded = false;
        poll_fn(|context| {
            if yielded {
                return Poll::Ready(());
            }
            yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }
}

/// Whether `error` is the trap of a suspending host function that answered
/// "not yet" where its call cannot suspend: [`TrapKind::Host`], with a
/// message that says so, on every backend.
fn cannot_suspend(error: &Error) -> bool {
    matches!(
        error,
        Error::Trap(TrapKind::Host(error)) if error.to_string().contains("cannot suspend")
    )
}

/// A store with the guest of the tests of how a resumable call ends
/// without a suspension, and its instance.
///
/// The suspending host function `answer` answers a negative argument at
/// once with its double, fails at zero, and records any other argument in
/// `log` and answers "not yet". `run` gives the result of `answer` and its
/// own argument. `fail` calls a host function that cannot suspend, and
/// fails.
async fn answers(engine: &Engine, log: &Log) -> (Store<()>, Instance) {
    let mut store = support::store(engine, ());
    let waits = log.clone();
    let answer = Func::new_suspending(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        move |_, params, results| {
            let argument = argument(params)?;
            if argument < 0 {
                results[0] = Val::I32(argument * 2);
                return Ok(Poll::Ready(()));
            }
            anyhow::ensure!(argument != 0, "no answer for zero");
            waits.push(Event::Waits(argument));
            Ok(Poll::Pending)
        },
    )
    .expect("the store makes a suspending host function");
    let fail = Func::new(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
        Err(anyhow::anyhow!("the host failed"))
    })
    .expect("the store makes a host function");
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (import "host" "answer" (func $answer (param i32) (result i32)))
              (import "host" "fail" (func $fail (result i32)))
              (func (export "run") (param i32) (result i32 i32)
                local.get 0
                call $answer
                local.get 0)
              (func (export "fail") (result i32)
                call $fail))
            "#
        ),
        &[answer.into(), fail.into()],
    )
    .await;
    (store, instance)
}

/// A suspending host function that answers at once does not suspend its
/// call, and the resumable call finishes with its results.
pub async fn it_finishes_a_resumable_call_that_does_not_suspend(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let log = Log::default();
    let (mut store, instance) = answers(engine, &log).await;
    let run = support::func(&mut store, instance, "run");

    let mut results = [Val::I32(0), Val::I32(0)];
    let outcome = run
        .call_resumable(&mut store, &[Val::I32(-4)], &mut results)
        .await;
    assert_eq!(finished(outcome, &results), [Some(-8), Some(-4)]);
    assert!(log.events().is_empty(), "nothing waited");
}

/// Outside a resumable call, a suspending host function that answers "not
/// yet" traps the call with [`TrapKind::Host`] and a message that says the
/// call cannot suspend. The store stays good: a resumable call suspends in
/// it afterwards, and finishes.
pub async fn it_traps_a_suspension_outside_a_resumable_call(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let log = Log::default();
    let (mut store, instance) = answers(engine, &log).await;
    let run = support::func(&mut store, instance, "run");

    let outcome = run.call(&mut store, &[Val::I32(7)], &mut [Val::I32(0), Val::I32(0)]);
    assert!(
        outcome.as_ref().err().is_some_and(cannot_suspend),
        "{outcome:?}"
    );

    let mut results = [Val::I32(0), Val::I32(0)];
    let outcome = run
        .call_resumable(&mut store, &[Val::I32(8)], &mut results)
        .await;
    let handle = waiting(outcome);
    let mut results = [Val::I32(0), Val::I32(0)];
    let outcome = handle
        .resume(&mut store, &[Val::I32(80)], &mut results)
        .await;
    assert_eq!(finished(outcome, &results), [Some(80), Some(8)]);
    assert_eq!(log.events(), [Event::Waits(7), Event::Waits(8)]);
}

/// Inside a resumable call, an error of a host function traps the call
/// with the host's error, unchanged, and does not suspend it. That holds
/// for a host function that cannot suspend, and for a suspending one.
pub async fn it_traps_a_resumable_call_with_the_error_of_a_host_function(engine: &Engine) {
    if !support::declares(engine, &[Capability::HostSuspension]) {
        return;
    }
    let log = Log::default();
    let (mut store, instance) = answers(engine, &log).await;

    let fail = support::func(&mut store, instance, "fail");
    let outcome = fail
        .call_resumable(&mut store, &[], &mut [Val::I32(0)])
        .await;
    assert!(
        matches!(
            &outcome,
            Err(Error::Trap(TrapKind::Host(error))) if error.to_string() == "the host failed"
        ),
        "{outcome:?}"
    );

    let run = support::func(&mut store, instance, "run");
    let outcome = run
        .call_resumable(&mut store, &[Val::I32(0)], &mut [Val::I32(0), Val::I32(0)])
        .await;
    assert!(
        matches!(
            &outcome,
            Err(Error::Trap(TrapKind::Host(error))) if error.to_string() == "no answer for zero"
        ),
        "{outcome:?}"
    );
    assert!(log.events().is_empty(), "nothing waited");
}
