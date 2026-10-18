// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the waker of a host task's first poll.
//!
//! `Future` lets each poll carry a different waker, and asks a future
//! to wake the waker of its latest poll. Some futures keep the waker
//! of their first poll and wake only that one. `callback-future`
//! 0.1.0 does, and `wasi-webgpu-wasmtime` waits on wgpu-core's
//! callbacks through it.
//!
//! The store polls a host task's body once before the call returns to
//! the guest, and in later turns. Every one of those polls carries the
//! task's own waker, the first as well, so a wake of the first poll's
//! waker marks the task, and the next turn polls it. Each test below
//! holds a body that keeps the waker of its first poll and is
//! released by a callback that fires once the call is pending, as
//! wgpu-core's callback fires once the device's work is done. The
//! call resolves after the wake, through an asynchronous lower and
//! through a synchronous one that blocks the guest thread.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, Engine, Error, Func, Instance, Linker, SchedulerCause, Store,
    SuspendProviderKind, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component whose async-typed export calls the async-typed import
/// `answer` through an asynchronous lower and waits on its subtask.
///
/// `run` is lifted `async` with a callback. A call that returned at
/// once hands its result back; one that started joins its subtask to
/// a set and waits there, and the callback hands back the result the
/// lowering wrote once the subtask's event arrives.
const AN_ASYNCHRONOUS_LOWER: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A component whose async-typed export calls the async-typed import
/// `answer` through a synchronous lower and hands the result back
/// through `task.return`.
///
/// The export's task is one that may block, so a body that is still
/// pending blocks the guest thread where it stands, through the
/// store's suspend provider.
const A_SYNCHRONOUS_LOWER: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run") (param i32) (result i32)
          (call $task-return (call $answer (local.get 0)))
          (i32.const 0))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (unreachable)))
      (core instance $m (instantiate $m
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A callback the test fires once the call is pending, and the one
/// waker the body that waits on it kept.
#[derive(Clone, Default)]
struct Callback {
    fired: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
    polls: Arc<AtomicUsize>,
}

impl Callback {
    /// Fire the callback, which wakes the waker the body kept from
    /// its first poll, and only that one.
    fn fire(&self) {
        self.fired.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("the waker").as_ref() {
            waker.wake_by_ref();
        }
    }

    /// How many times the body was polled.
    fn polls(&self) -> usize {
        self.polls.load(Ordering::Acquire)
    }
}

/// A body that resolves once its callback fired, and keeps the waker
/// of its first poll alone, as `callback-future` 0.1.0 does.
struct KeepsTheFirstWaker {
    callback: Callback,
    value: u32,
}

impl Future for KeepsTheFirstWaker {
    type Output = Result<(u32,), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.callback.polls.fetch_add(1, Ordering::AcqRel);
        if self.callback.fired.load(Ordering::Acquire) {
            return Poll::Ready(Ok((self.value,)));
        }
        self.callback
            .waker
            .lock()
            .expect("the waker")
            .get_or_insert_with(|| context.waker().clone());
        Poll::Pending
    }
}

/// Instantiate `binary` with an `answer` whose body waits on
/// `callback` and then doubles its argument.
async fn instantiate(binary: &[u8], callback: &Callback) -> (Engine, Store<()>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let callback = callback.clone();
    linker
        .root()
        .func_wrap_concurrent("answer", move |_accessor: &Accessor<()>, (x,): (u32,)| {
            KeepsTheFirstWaker {
                callback: callback.clone(),
                value: x * 2,
            }
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (engine, store, instance)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
}

/// How many polls of the call the tests allow after the callback
/// fired. A wake that reaches the task resolves the call within a
/// few; a lost one leaves it pending however often it is polled.
const POLLS_AFTER_THE_WAKE: usize = 16;

/// Poll `call` once with the waker of the test's task.
async fn poll_once<F: Future + Unpin>(call: &mut F) -> Poll<F::Output> {
    poll_fn(|context| Poll::Ready(Pin::new(&mut *call).poll(context))).await
}

/// Give the page's event loop a turn, which is where a thread the
/// browser's provider suspended resumes. Natively there is nothing to
/// wait for.
async fn yield_to_the_host() {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(
        &wasm_bindgen::JsValue::UNDEFINED,
    ))
    .await
    .expect("a resolved promise");
}

/// Poll `call` after the callback fired until it resolves, and answer
/// what it resolved with.
async fn resolve_after_the_wake<F: Future + Unpin>(call: &mut F) -> F::Output {
    for _ in 0..POLLS_AFTER_THE_WAKE {
        if let Poll::Ready(outcome) = poll_once(call).await {
            return outcome;
        }
        yield_to_the_host().await;
    }
    panic!(
        "the call was still pending {POLLS_AFTER_THE_WAKE} polls after the callback woke \
         the waker of the body's first poll, so the wake never reached the task"
    );
}

#[wcmp_macros::test]
async fn it_resolves_an_asynchronous_lower_whose_body_wakes_the_waker_of_its_first_poll() {
    let callback = Callback::default();
    let (_engine, mut store, instance) = instantiate(AN_ASYNCHRONOUS_LOWER, &callback).await;
    let run = func(&instance, "run");
    let mut call = Box::pin(run.call(&mut store, &[Val::U32(21)]));

    assert!(
        poll_once(&mut call).await.is_pending(),
        "the body waits on its callback, so the call is pending"
    );
    let polls = callback.polls();
    assert!(
        polls >= 1,
        "the call polled the body before it waited on the subtask"
    );

    callback.fire();
    let result = resolve_after_the_wake(&mut call)
        .await
        .expect("the call resolves");
    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the body's result reached the guest"
    );
    assert_eq!(
        callback.polls(),
        polls + 1,
        "the wake brought one more poll of the body, which resolved it"
    );
}

#[wcmp_macros::test]
async fn it_resolves_a_blocking_lower_whose_body_wakes_the_waker_of_its_first_poll() {
    let callback = Callback::default();
    let (engine, mut store, instance) = instantiate(A_SYNCHRONOUS_LOWER, &callback).await;
    let run = func(&instance, "run");
    let mut call = Box::pin(run.call(&mut store, &[Val::U32(21)]));
    let first = poll_once(&mut call).await;

    // With no provider the block runs nested turns above the call,
    // which give no control back to the host, so nothing can fire the
    // callback while the call waits: the call fails with the
    // stack-switch cause.
    if engine.suspend_provider() == SuspendProviderKind::None {
        let Poll::Ready(Err(error)) = first else {
            panic!("with no provider the first poll ends the call, got {first:?}");
        };
        let cause = SchedulerCause::StackSwitchNeeded.to_string();
        assert!(
            format!("{error} {error:?}").contains(&cause)
                || format!("{error:?}").contains("StackSwitchNeeded"),
            "the block failed with the stack-switch cause, got {error:?}"
        );
        return;
    }
    assert!(
        first.is_pending(),
        "the body waits on its callback, so the guest thread is blocked in the call, \
         got {first:?}"
    );

    callback.fire();
    let result = resolve_after_the_wake(&mut call)
        .await
        .expect("the call resolves");
    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the body's result reached the guest as the synchronous lower returned"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
