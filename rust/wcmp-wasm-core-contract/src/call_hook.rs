// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The call hook of a store.

use core::task::Poll;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    CallHook, Caller, Engine, Error, Func, FuncType, Instance, ResumableCall, Store, TrapKind, Val,
    ValType,
};

use crate::support;

/// A module whose `run` notes its argument through the host function
/// `note` and answers it, whose `trap` traps, and whose `outer` calls
/// `run` back through the host function `through`.
const NOTE: &[u8] = wasm!(
    r#"
    (module
      (import "host" "note" (func $note (param i32)))
      (import "host" "through" (func $through (param i32) (result i32)))
      (func (export "run") (param i32) (result i32)
        local.get 0
        call $note
        local.get 0)
      (func (export "trap")
        unreachable)
      (func (export "outer") (param i32) (result i32)
        local.get 0
        call $through))
    "#
);

/// A module whose `run` waits in the suspending host function `wait`,
/// notes what the wait answered through `note`, and answers it.
const WAIT: &[u8] = wasm!(
    r#"
    (module
      (import "host" "wait" (func $wait (param i32) (result i32)))
      (import "host" "note" (func $note (param i32)))
      (func (export "run") (param i32) (result i32)
        (local $resumed i32)
        local.get 0
        call $wait
        local.tee $resumed
        call $note
        local.get $resumed))
    "#
);

/// The crossings a hook saw, in order.
#[derive(Clone, Default)]
struct Crossings(Arc<Mutex<Vec<CallHook>>>);

impl Crossings {
    /// Records `kind`.
    fn push(&self, kind: CallHook) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(kind);
    }

    /// Every crossing since the last take, and none after.
    fn take(&self) -> Vec<CallHook> {
        core::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The host function `note` of type `[i32] -> []`, which counts its calls
/// in `notes`.
fn note<T: 'static>(store: &mut Store<T>, notes: &Arc<AtomicUsize>) -> Func {
    let notes = notes.clone();
    Func::new(
        &mut *store,
        FuncType::new([ValType::I32], []),
        move |_, _, _| {
            notes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect("the store makes a host function")
}

/// What the store of a test of the call hook holds: how many crossings
/// the hook saw, and the guest function that the host function `through`
/// calls back into.
#[derive(Default)]
struct Hooked {
    crossings: usize,
    inner: Option<Func>,
}

/// The host function `through` of type `[i32] -> [i32]`, which calls the
/// guest function the store holds with its argument.
fn through(store: &mut Store<Hooked>) -> Func {
    Func::new(
        &mut *store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |mut caller: Caller<'_, Hooked>, params, results| {
            let inner = caller
                .data()
                .inner
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            inner.call(&mut caller, params, results)?;
            Ok(())
        },
    )
    .expect("the store makes a host function")
}

/// An instance of [`NOTE`] in `store`, with `note` counting its calls in
/// `notes`, and its `run` as the function `through` calls.
async fn noted(store: &mut Store<Hooked>, notes: &Arc<AtomicUsize>) -> Instance {
    let note = note(store, notes);
    let through = through(store);
    let instance = support::instance(store, NOTE, &[note.into(), through.into()]).await;
    let run = support::func(store, instance, "run");
    store.data_mut().inner = Some(run);
    instance
}

/// `crossings` without the stretches of guest code that crossed nothing,
/// where a `CallingWasm` comes straight before its `ReturningFromWasm`.
fn without_empty_stretches(crossings: &[CallHook]) -> Vec<CallHook> {
    let mut kept: Vec<CallHook> = Vec::new();
    for &kind in crossings {
        if kind == CallHook::ReturningFromWasm && kept.last() == Some(&CallHook::CallingWasm) {
            kept.pop();
        } else {
            kept.push(kind);
        }
    }
    kept
}

/// The engine calls the store's hook, with the store, at each crossing
/// between the host and guest code: around an instantiation, around a
/// call, and around the host function the call calls, at any depth. A
/// call that traps still returns to the host. A hook set later takes the
/// place of the first, and so does a hook set from inside the hook.
pub async fn it_calls_the_call_hook_at_each_crossing_between_host_and_guest(engine: &Engine) {
    let mut store = support::store(engine, Hooked::default());
    let crossings = Crossings::default();
    let seen = crossings.clone();
    store.call_hook(move |mut context, kind| {
        context.data_mut().crossings += 1;
        seen.push(kind);
        Ok(())
    });
    let notes = Arc::new(AtomicUsize::new(0));
    let instance = noted(&mut store, &notes).await;
    assert_eq!(
        crossings.take(),
        [CallHook::CallingWasm, CallHook::ReturningFromWasm],
        "the engine crosses around an instantiation, whether or not the module has a start \
         function"
    );

    let run = support::func(&mut store, instance, "run");
    let results = support::call(&mut store, run, &[Val::I32(7)], &[ValType::I32]);
    assert!(matches!(results.as_slice(), [Val::I32(7)]));
    assert_eq!(
        crossings.take(),
        [
            CallHook::CallingWasm,
            CallHook::CallingHost,
            CallHook::ReturningFromHost,
            CallHook::ReturningFromWasm,
        ]
    );
    assert_eq!(notes.load(Ordering::SeqCst), 1);

    let trap = support::func(&mut store, instance, "trap");
    let trapped = trap.call(&mut store, &[], &mut []);
    assert!(
        matches!(trapped, Err(Error::Trap(TrapKind::UnreachableCodeReached))),
        "{trapped:?}"
    );
    assert_eq!(
        crossings.take(),
        [CallHook::CallingWasm, CallHook::ReturningFromWasm],
        "a trap returns to the host too"
    );
    let outer = support::func(&mut store, instance, "outer");
    let results = support::call(&mut store, outer, &[Val::I32(8)], &[ValType::I32]);
    assert!(matches!(results.as_slice(), [Val::I32(8)]));
    assert_eq!(
        crossings.take(),
        [
            CallHook::CallingWasm,
            CallHook::CallingHost,
            CallHook::CallingWasm,
            CallHook::CallingHost,
            CallHook::ReturningFromHost,
            CallHook::ReturningFromWasm,
            CallHook::ReturningFromHost,
            CallHook::ReturningFromWasm,
        ],
        "a host function that calls back into the guest nests a stretch of its own"
    );
    assert_eq!(
        store.data().crossings,
        16,
        "the hook reached the host's data at each crossing"
    );

    store.call_hook(|_, _| Ok(()));
    support::call(&mut store, run, &[Val::I32(9)], &[ValType::I32]);
    assert_eq!(
        crossings.take(),
        [],
        "the later hook took the first one's place"
    );

    let replaced = crossings.clone();
    store.call_hook(move |mut context, _| {
        let seen = replaced.clone();
        context.call_hook(move |_, kind| {
            seen.push(kind);
            Ok(())
        });
        Ok(())
    });
    support::call(&mut store, run, &[Val::I32(10)], &[ValType::I32]);
    assert_eq!(
        crossings.take(),
        [
            CallHook::CallingHost,
            CallHook::ReturningFromHost,
            CallHook::ReturningFromWasm,
        ],
        "a hook set from inside the hook ran from the next crossing on"
    );
}

/// An error from the hook takes the place of what follows it. Before the
/// guest runs, the call fails with the error and the guest does not run.
/// Before a host function's body, the body does not run, and the guest
/// traps with the error. After the body or the guest stopped, the error
/// takes the place of what they answered.
pub async fn it_fails_a_call_with_the_error_of_its_call_hook(engine: &Engine) {
    let mut store = support::store(engine, Hooked::default());
    let notes = Arc::new(AtomicUsize::new(0));
    let instance = noted(&mut store, &notes).await;
    let run = support::func(&mut store, instance, "run");

    for (refused, noted) in [
        (CallHook::CallingWasm, 0),
        (CallHook::CallingHost, 0),
        (CallHook::ReturningFromHost, 1),
        (CallHook::ReturningFromWasm, 1),
    ] {
        store.call_hook(move |_, kind| {
            if kind == refused {
                Err(Error::Backend {
                    message: "the hook refuses".to_owned(),
                })
            } else {
                Ok(())
            }
        });
        notes.store(0, Ordering::SeqCst);
        let mut results = [Val::I32(0)];
        let outcome = run.call(&mut store, &[Val::I32(7)], &mut results);
        let refusal = |error: &Error| matches!(error, Error::Backend { message } if message == "the hook refuses");
        let failed = match &outcome {
            Err(Error::Trap(TrapKind::Host(error)))
                if refused == CallHook::CallingHost || refused == CallHook::ReturningFromHost =>
            {
                error.downcast_ref::<Error>().is_some_and(refusal)
            }
            Err(error) => refusal(error),
            Ok(()) => false,
        };
        assert!(
            failed,
            "a hook that refuses {refused:?} fails the call with its error, got {outcome:?}"
        );
        assert_eq!(
            notes.load(Ordering::SeqCst),
            noted,
            "the host function ran {noted} times where the hook refused {refused:?}"
        );
    }
}

/// A resumable call that suspends in a suspending host function crosses
/// back to the host with `ReturningFromWasm`, and the suspending function
/// does not return to the guest. Each resumption is a stretch of guest code
/// of its own, in one step and in two.
pub async fn it_calls_the_call_hook_around_each_stretch_of_a_resumable_call(engine: &Engine) {
    if support::refuses_host_suspension(engine) {
        return;
    }
    let mut store = support::store(engine, ());
    let notes = Arc::new(AtomicUsize::new(0));
    let wait = Func::new_suspending(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |_, _, _| Ok(Poll::Pending),
    )
    .expect("the store makes a suspending host function");
    let note = note(&mut store, &notes);
    let instance = support::instance(&mut store, WAIT, &[wait.into(), note.into()]).await;
    let run = support::func(&mut store, instance, "run");
    let crossings = Crossings::default();
    let seen = crossings.clone();
    store.call_hook(move |_, kind| {
        seen.push(kind);
        Ok(())
    });
    let suspended = [
        CallHook::CallingWasm,
        CallHook::CallingHost,
        CallHook::ReturningFromWasm,
    ];
    let resumed = [
        CallHook::CallingWasm,
        CallHook::CallingHost,
        CallHook::ReturningFromHost,
        CallHook::ReturningFromWasm,
    ];

    let mut results = [Val::I32(0)];
    let outcome = run
        .call_resumable(&mut store, &[Val::I32(1)], &mut results)
        .await;
    let Ok(ResumableCall::Suspended(handle)) = outcome else {
        panic!("the call waits: {outcome:?}");
    };
    assert_eq!(crossings.take(), suspended);
    let outcome = handle
        .resume(&mut store, &[Val::I32(7)], &mut results)
        .await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert!(matches!(results, [Val::I32(7)]));
    assert_eq!(crossings.take(), resumed);

    let outcome = run
        .call_resumable(&mut store, &[Val::I32(2)], &mut results)
        .await;
    let Ok(ResumableCall::Suspended(handle)) = outcome else {
        panic!("the call waits: {outcome:?}");
    };
    assert_eq!(crossings.take(), suspended);
    let mut resumption = handle
        .start_resume(&mut store, &[Val::I32(9)])
        .expect("the call resumes");
    let outcome = resumption.stop(&mut store, &mut results).await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert!(matches!(results, [Val::I32(9)]));
    // A backend runs the rest of the call in the start of the resumption
    // or in the wait, and the other of the two crosses nothing.
    assert_eq!(without_empty_stretches(&crossings.take()), resumed);
    assert_eq!(notes.load(Ordering::SeqCst), 2);
}
