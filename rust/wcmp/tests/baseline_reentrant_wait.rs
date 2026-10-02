// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for two caller instances that synchronously lower
//! the same asynchronous callee export.
//!
//! A synchronous lower of an asynchronously lifted export goes
//! through a fused adapter, and the adapter reaches the callee
//! through the `sync-start-call` intrinsic. That intrinsic names no
//! component instance, only the callee's callback, so the translator
//! makes one host function of it for every caller of that callee.
//! When the callee parks and no suspend provider is filled, the
//! intrinsic runs a nested turn from inside itself, so its host
//! function stays on the stack while the turn runs. A call of a
//! second caller instance that the turn starts, and that lowers the
//! same export, calls that one host function a second time. Neither
//! caller re-enters anything of its own, and each caller instance
//! runs one task only, so the entry rules of the model let both calls
//! through.
//!
//! Both backends enter a host function at any depth, so the second
//! call blocks in a nested turn of its own and both calls return on
//! both targets.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, Engine, HostCall, Linker, Result, Store, SuspendProviderKind, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component with one asynchronous callee and two instances of one
/// caller that synchronously lowers it.
///
/// The callee `$A` lifts `work` with a callback. `work` calls the host
/// `async` import `answer` through an asynchronous lower, joins the
/// subtask to a fresh set, and returns the wait word naming that set.
/// The callback answers with what the host returned and exits. Each
/// call has the host write its result at an address drawn from its
/// argument, and records that address under its subtask's index, so
/// two calls in flight at once never share memory.
///
/// The caller `$B` logs its argument, calls `work` through a
/// synchronous lower, logs one higher, and answers with what `work`
/// returned. The outer component instantiates it twice over the same
/// `work` and exports the two `run`s as `first` and `second`. Both
/// adapters reach the callee through the one `sync-start-call`
/// intrinsic its callback names.
const TWO_CALLERS_OF_ONE_CALLEE: &[u8] = component!(
    r#"
    (component
      (import "log" (func $log (param "x" u32)))
      (import "answer" (func $answer async (param "x" u32) (result u32)))

      (component $A
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $answer
          (canon lower (func $answer) async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core func $subtask-drop (canon subtask.drop))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "work") (param $x i32) (result i32)
            (local $subtask i32)
            (local $set i32)
            (local.set $subtask
              (i32.shr_u
                (call $answer (local.get $x) (i32.shl (local.get $x) (i32.const 3)))
                (i32.const 4)))
            (i32.store
              (i32.add (i32.const 1024) (i32.shl (local.get $subtask) (i32.const 2)))
              (i32.shl (local.get $x) (i32.const 3)))
            (local.set $set (call $set-new))
            (call $join (local.get $subtask) (local.get $set))
            (i32.or (i32.const 2) (i32.shl (local.get $set) (i32.const 4))))
          (func (export "callback")
            (param $event i32) (param $subtask i32) (param $payload i32) (result i32)
            (call $subtask-drop (local.get $subtask))
            (call $task-return
              (i32.load
                (i32.load
                  (i32.add (i32.const 1024) (i32.shl (local.get $subtask) (i32.const 2))))))
            (i32.const 0)))
        (core instance $m (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "answer" (func $answer))
            (export "waitable-set.new" (func $set-new))
            (export "waitable.join" (func $join))
            (export "subtask.drop" (func $subtask-drop))
            (export "task.return" (func $task-return))))))
        (func (export "work") async (param "x" u32) (result u32)
          (canon lift (core func $m "work") async (callback (core func $m "callback")))))

      (component $B
        (import "log" (func $log (param "x" u32)))
        (import "work" (func $work async (param "x" u32) (result u32)))
        (core func $log (canon lower (func $log)))
        (core func $work (canon lower (func $work)))
        (core module $m
          (import "" "log" (func $log (param i32)))
          (import "" "work" (func $work (param i32) (result i32)))
          (func (export "run") (param $x i32) (result i32)
            (local $answer i32)
            (call $log (local.get $x))
            (local.set $answer (call $work (local.get $x)))
            (call $log (i32.add (local.get $x) (i32.const 1)))
            (local.get $answer)))
        (core instance $m (instantiate $m
          (with "" (instance
            (export "log" (func $log))
            (export "work" (func $work))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $m "run"))))

      (instance $a (instantiate $A (with "answer" (func $answer))))
      (instance $first (instantiate $B
        (with "log" (func $log))
        (with "work" (func $a "work"))))
      (instance $second (instantiate $B
        (with "log" (func $log))
        (with "work" (func $a "work"))))
      (export "first" (func $first "run"))
      (export "second" (func $second "run")))
    "#
);

/// What the guest logged, in the order it logged it.
type Log = Arc<Mutex<Vec<u32>>>;

/// A future that is pending on its first poll and ready with its
/// value on the next.
///
/// The asynchronous lower's own poll is the first, so the callee's
/// call starts a subtask and the callee has something to wait on; a
/// later turn's poll completes it.
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
    type Output = Result<V>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.polled {
            let value = this.value.take().expect("the future is polled once ready");
            return Poll::Ready(Ok(value));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// What the first call answered, what the second call answered, and
/// what the guest logged.
type Outcome = (Result<Box<[Val]>>, Result<Box<[Val]>>, Vec<u32>);

/// Instantiate [`TWO_CALLERS_OF_ONE_CALLEE`], call `first` with 1 and
/// `second` with 10 at once, and hand back the [`Outcome`].
///
/// The host `answer` doubles its argument, one poll late. The first
/// call blocks in `sync-start-call`, and the nested turn its block
/// opens runs the second call's start, so the second call reaches
/// that intrinsic while the first call's block is still on the stack.
///
/// The polling stops once both calls have resolved.
async fn two_callers() -> Outcome {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, TWO_CALLERS_OF_ONE_CALLEE)
        .await
        .expect("component parses");
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
        .expect("the log registration");
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
            PendingOnce::new(x * 2)
        })
        .expect("the answer registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let first = instance.get_func("first").expect("first export");
    let second = instance.get_func("second").expect("second export");

    let (first, second) = store
        .run_concurrent(async |accessor| {
            let first_args = [Val::U32(1)];
            let second_args = [Val::U32(10)];
            let mut first = Box::pin(first.call_concurrent(accessor, &first_args));
            let mut second = Box::pin(second.call_concurrent(accessor, &second_args));
            let mut first_done: Option<Result<Box<[Val]>>> = None;
            let mut second_done: Option<Result<Box<[Val]>>> = None;

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
                if first_done.is_some() && second_done.is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;

            (
                first_done.expect("the first call resolved"),
                second_done.expect("the second call resolved"),
            )
        })
        .await
        .expect("run the closure");

    let entries = log.lock().expect("log").clone();
    (first, second, entries)
}

/// What the two calls answer: the second call's `sync-start-call`
/// opens a nested turn of its own, which completes its callee's host
/// call, and both calls return.
///
/// Under a suspend provider neither call opens a nested turn. Each
/// caller's thread runs on a stack of its own and suspends in its
/// `sync-start-call` until its callee returns, so the second call
/// starts while the first is suspended, and each caller logs its
/// second entry once its own callee has answered.
#[wcmp_macros::test]
async fn it_runs_two_callers_that_synchronously_lower_one_async_export() {
    let provider = Engine::with_backend(crate::test_backend::backend())
        .expect("engine")
        .suspend_provider()
        != SuspendProviderKind::None;
    let (first, second, log) = two_callers().await;

    assert_eq!(
        first.expect("the first call resolves").as_ref(),
        [Val::U32(2)],
        "the first caller's lower returned with the callee's answer"
    );
    assert_eq!(
        second.expect("the second call resolves").as_ref(),
        [Val::U32(20)],
        "both backends call `sync-start-call` while a call of it is \
         still on the stack, so the second caller's lower returned as well"
    );
    if provider {
        assert_eq!(
            log,
            vec![1, 10, 2, 11],
            "the second call started while the first was suspended, and each \
             returned once its callee answered"
        );
    } else {
        assert_eq!(
            log,
            vec![1, 10, 11, 2],
            "the second call started and returned inside the first call's block"
        );
    }
}

#[path = "support/backend.rs"]
mod test_backend;
