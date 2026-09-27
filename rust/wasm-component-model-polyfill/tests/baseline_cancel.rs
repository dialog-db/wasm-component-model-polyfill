//! Baseline tests for the two cancellation built-ins, `task.cancel`
//! and `subtask.cancel`.
//!
//! Cancellation is cooperative. A caller asks a subtask to stop with
//! `subtask.cancel`, and the callee decides: it confirms with
//! `task.cancel`, or returns a result all the same. The cancel
//! answers the resolution the callee reached, or `BLOCKED` when an
//! asynchronous cancel finds the callee still running.
//!
//! The component below pairs a callee with a caller. Each export of
//! the caller makes one call into the callee through an asynchronous
//! lower, cancels it, and answers what the cancel answered, trapping
//! wherever the result the call left in memory is not the one the
//! resolution promises:
//!
//! - A callee its instance's entry gate holds never runs, and the
//!   cancel answers `CANCELLED_BEFORE_STARTED` at once. The gate lets
//!   the next call through once backpressure clears, so the cancelled
//!   start leaves no place in the queue behind.
//! - A callback callee waiting in its loop takes the request at once,
//!   as the task-cancelled event: one that confirms resolves to
//!   `CANCELLED_BEFORE_RETURNED`, and one that returns a result
//!   instead resolves to `RETURNED`.
//! - A stackful callee has no built-in that takes the request, so it
//!   is never told. An asynchronous cancel gives way once and answers
//!   `BLOCKED`, and a synchronous one waits until the callee returns
//!   on its own.
//! - A request comes before an event the callee's set already holds
//!   when the request arrives. The set keeps that event for the
//!   callee's next wait.
//! - A woken callee that blocks before it confirms suspends above the
//!   cancel that gave way to it, and the cancel still finishes.
//! - A woken callee that switches to a suspended thread of its own
//!   task before it confirms runs that thread, and confirms once the
//!   thread made it ready again.
//!
//! A caller that lent a borrow for the call gets the lend back when
//! the cancel delivers the resolution, whichever resolution it is, and
//! not before. A call the entry gate holds has lent nothing yet, so
//! its cancel has no lend to give back. A callee that still holds the
//! borrow cannot confirm.
//!
//! A callback queued to take an event its set holds takes the event
//! only as it runs, which is what lets a request come first. The set
//! counts the callback's task as a waiter until then, so a drop of the
//! set in the meantime traps rather than pulling the set out from
//! under the callback, and the callback can drop the set once it has
//! its event.
//!
//! The failures each built-in raises that the conformance corpus does
//! not reach are read back here with their messages: a second cancel
//! of one subtask, which is the polyfill's own message; a cancel after
//! the resolution was delivered; a `task.cancel` in a task that was
//! never cancelled, whatever its lift; a second resolution of a
//! cancelled task; and a `task.cancel` with a borrow outstanding.
//!
//! The may-leave check comes first for both built-ins. A call while
//! the instance's may-leave flag is clear, as it is during a
//! `post-return`, fails with the cannot-leave cause.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasm_component_model_polyfill::{
    Accessor, Component, Engine, EngineConfig, Error, Instance, Linker, Store, TaskCause, Val,
    WaitableCause,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The subtask state of a callee that returned its result.
const RETURNED: u32 = 2;
/// The subtask state of a callee cancelled before it started.
const CANCELLED_BEFORE_STARTED: u32 = 3;
/// The subtask state of a callee that confirmed its cancellation.
const CANCELLED_BEFORE_RETURNED: u32 = 4;
/// What an asynchronous cancel answers when the callee has not
/// resolved.
const BLOCKED: u32 = 0xffff_ffff;

/// Which built-in the `post-return` of `run` calls, chosen by the
/// `select` export before the call. Zero calls neither.
const NEITHER: u32 = 0;
/// Call `task.cancel`.
const TASK_CANCEL: u32 = 1;
/// Call `subtask.cancel`.
const SUBTASK_CANCEL: u32 = 2;

/// A component that imports both cancellation built-ins and cancels
/// nothing.
///
/// `answer` never cancels. The `post-return` of `run` calls the
/// built-in `select` chose, with the instance's may-leave flag clear.
const CANCELS: &[u8] = component!(
    r#"
    (component
      (core func $task-cancel (canon task.cancel))
      (core func $subtask-cancel (canon subtask.cancel))
      (core module $m
        (import "" "task.cancel" (func $task-cancel))
        (import "" "subtask.cancel" (func $subtask-cancel (param i32) (result i32)))
        (global $which (mut i32) (i32.const 0))
        (func (export "answer") (result i32) (i32.const 42))
        (func (export "select") (param i32) (global.set $which (local.get 0)))
        (func (export "run") (result i32) (i32.const 5))
        (func (export "post-return") (param i32)
          (if (i32.eq (global.get $which) (i32.const 1))
            (then (call $task-cancel)))
          (if (i32.eq (global.get $which) (i32.const 2))
            (then (drop (call $subtask-cancel (i32.const 1)))))))
      (core instance $i (instantiate $m (with "" (instance
        (export "task.cancel" (func $task-cancel))
        (export "subtask.cancel" (func $subtask-cancel))))))
      (func (export "answer") (result u32) (canon lift (core func $i "answer")))
      (func (export "select") (param "w" u32) (canon lift (core func $i "select")))
      (func (export "run") (result u32)
        (canon lift (core func $i "run")
          (post-return (core func $i "post-return")))))
    "#
);

/// A callee `$C` and a caller `$D` that cancels calls into it.
///
/// Every export of `$C` but the three helpers is lifted `async`. The
/// callback ones park in their loop on a set that never takes an
/// event, so only a cancellation request wakes them, and each does
/// one thing with the task-cancelled event: confirm, return `7`
/// instead, ignore it and park again, confirm twice, or return and
/// then confirm. `confirm-at-once` confirms before any request.
/// `stackful` yields ten times, polling the empty set each time, and
/// then returns `7`; a request would show as event 6 of neither
/// built-in, so the traps it makes on a non-zero answer are what say
/// it was never told.
///
/// Each export of `$D` makes one asynchronous call into `$C` with its
/// result pointer at address 0, checks the state the lower answered,
/// cancels, and answers what the cancel answered.
const CALLS: &[u8] = component!(
    r#"
    (component
      (component $C
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core module $CM
          (import "" "mem" (memory 1))
          (import "" "task.cancel" (func $task.cancel))
          (import "" "task.return" (func $task.return (param i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.poll" (func $waitable-set.poll (param i32 i32) (result i32)))
          (import "" "thread.yield" (func $thread.yield (result i32)))
          (import "" "backpressure.inc" (func $backpressure.inc))
          (import "" "backpressure.dec" (func $backpressure.dec))
          (global $ws (mut i32) (i32.const 0))
          (func $start (global.set $ws (call $waitable-set.new)))
          (start $start)
          (func $park (export "park") (result i32)
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $ws) (i32.const 4))))
          (func $expect-cancelled (param $event i32)
            (if (i32.ne (local.get $event) (i32.const 6 (; TASK_CANCELLED ;)))
              (then unreachable)))
          (func (export "confirm-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "return-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $task.return (i32.const 7))
            (i32.const 0 (; EXIT ;)))
          (func (export "ignore-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $park))
          (func (export "confirm-twice-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $task.cancel)
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "return-then-confirm-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $task.return (i32.const 7))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "unreachable-cb") (param i32 i32 i32) (result i32)
            unreachable)
          (func (export "confirm-at-once") (result i32)
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "quick") (result i32)
            (call $task.return (i32.const 5))
            (i32.const 0 (; EXIT ;)))
          (func (export "stackful")
            (local $i i32)
            (loop $again
              (if (i32.ne (call $thread.yield) (i32.const 0))
                (then unreachable))
              (if (i32.ne (call $waitable-set.poll (global.get $ws) (i32.const 16))
                          (i32.const 0 (; NONE ;)))
                (then unreachable))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br_if $again (i32.lt_u (local.get $i) (i32.const 10))))
            (call $task.return (i32.const 7)))
          (func (export "sync-confirm") (call $task.cancel))
          (func (export "set-backpressure") (call $backpressure.inc))
          (func (export "clear-backpressure") (call $backpressure.dec)))
        (canon task.cancel (core func $task.cancel))
        (canon task.return (result u32) (core func $task.return))
        (canon waitable-set.new (core func $waitable-set.new))
        (canon waitable-set.poll (memory (core memory $memory "mem")) (core func $waitable-set.poll))
        (canon thread.yield (core func $thread.yield))
        (canon backpressure.inc (core func $backpressure.inc))
        (canon backpressure.dec (core func $backpressure.dec))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "task.cancel" (func $task.cancel))
          (export "task.return" (func $task.return))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.poll" (func $waitable-set.poll))
          (export "thread.yield" (func $thread.yield))
          (export "backpressure.inc" (func $backpressure.inc))
          (export "backpressure.dec" (func $backpressure.dec))))))
        (func (export "park-confirm") async (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "confirm-cb"))))
        (func (export "park-return") async (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "return-cb"))))
        (func (export "park-ignore") async (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "ignore-cb"))))
        (func (export "park-confirm-twice") async (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "confirm-twice-cb"))))
        (func (export "park-return-then-confirm") async (result u32)
          (canon lift (core func $cm "park") async
            (callback (core func $cm "return-then-confirm-cb"))))
        (func (export "confirm-at-once") async (result u32)
          (canon lift (core func $cm "confirm-at-once") async
            (callback (core func $cm "unreachable-cb"))))
        (func (export "quick") async (result u32)
          (canon lift (core func $cm "quick") async (callback (core func $cm "unreachable-cb"))))
        (func (export "stackful") async (result u32)
          (canon lift (core func $cm "stackful") async))
        (func (export "sync-confirm") (canon lift (core func $cm "sync-confirm")))
        (func (export "set-backpressure") (canon lift (core func $cm "set-backpressure")))
        (func (export "clear-backpressure") (canon lift (core func $cm "clear-backpressure"))))

      (component $D
        (import "park-confirm" (func $park-confirm async (result u32)))
        (import "park-return" (func $park-return async (result u32)))
        (import "park-ignore" (func $park-ignore async (result u32)))
        (import "park-confirm-twice" (func $park-confirm-twice async (result u32)))
        (import "park-return-then-confirm" (func $park-return-then-confirm async (result u32)))
        (import "confirm-at-once" (func $confirm-at-once async (result u32)))
        (import "quick" (func $quick async (result u32)))
        (import "stackful" (func $stackful async (result u32)))
        (import "set-backpressure" (func $set-backpressure))
        (import "clear-backpressure" (func $clear-backpressure))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "cancel-sync" (func $cancel-sync (param i32) (result i32)))
          (import "" "subtask.drop" (func $subtask.drop (param i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "park-confirm" (func $park-confirm (param i32) (result i32)))
          (import "" "park-return" (func $park-return (param i32) (result i32)))
          (import "" "park-ignore" (func $park-ignore (param i32) (result i32)))
          (import "" "park-confirm-twice" (func $park-confirm-twice (param i32) (result i32)))
          (import "" "park-return-then-confirm"
            (func $park-return-then-confirm (param i32) (result i32)))
          (import "" "confirm-at-once" (func $confirm-at-once (param i32) (result i32)))
          (import "" "quick" (func $quick (param i32) (result i32)))
          (import "" "stackful" (func $stackful (param i32) (result i32)))
          (import "" "set-backpressure" (func $set-backpressure))
          (import "" "clear-backpressure" (func $clear-backpressure))

          ;; The subtask index a lower's status word carries, once the
          ;; state in its low bits is the one expected.
          (func $subtask (param $status i32) (param $state i32) (result i32)
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (local.get $state))
              (then unreachable))
            (i32.shr_u (local.get $status) (i32.const 4)))
          (func $started (param $status i32) (result i32)
            (call $subtask (local.get $status) (i32.const 1 (; STARTED ;))))
          (func $expect-result (param $value i32)
            (if (i32.ne (i32.load (i32.const 0)) (local.get $value))
              (then unreachable)))

          (func (export "cancel-at-gate") (result i32)
            (local $sub i32) (local $status i32)
            (call $set-backpressure)
            (local.set $sub
              (call $subtask (call $park-confirm (i32.const 0)) (i32.const 0 (; STARTING ;))))
            (local.set $status (call $cancel-async (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (call $clear-backpressure)
            ;; The gate lets the next call through at once.
            (if (i32.ne (call $quick (i32.const 0)) (i32.const 2 (; RETURNED ;)))
              (then unreachable))
            (call $expect-result (i32.const 5))
            (local.get $status))

          (func (export "cancel-confirming") (result i32)
            (local $sub i32) (local $status i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $sub (call $started (call $park-confirm (i32.const 0))))
            (local.set $status (call $cancel-async (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (call $expect-result (i32.const 0))
            (local.get $status))

          (func (export "cancel-returning") (result i32)
            (local $sub i32) (local $status i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $sub (call $started (call $park-return (i32.const 0))))
            (local.set $status (call $cancel-sync (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (call $expect-result (i32.const 7))
            (local.get $status))

          (func (export "cancel-stackful-async") (result i32)
            (local $sub i32) (local $ws i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $sub (call $started (call $stackful (i32.const 0))))
            (if (i32.ne (call $cancel-async (local.get $sub)) (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (local.set $ws (call $waitable-set.new))
            (call $waitable.join (local.get $sub) (local.get $ws))
            (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                        (i32.const 1 (; SUBTASK ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 8)) (local.get $sub))
              (then unreachable))
            (call $expect-result (i32.const 7))
            (i32.load (i32.const 12)))

          (func (export "cancel-stackful-sync") (result i32)
            (local $status i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $status (call $cancel-sync (call $started (call $stackful (i32.const 0)))))
            (call $expect-result (i32.const 7))
            (local.get $status))

          (func (export "cancel-twice") (result i32)
            (local $sub i32)
            (local.set $sub (call $started (call $park-ignore (i32.const 0))))
            (if (i32.ne (call $cancel-async (local.get $sub)) (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (call $cancel-async (local.get $sub)))

          (func (export "cancel-after-terminal") (result i32)
            (local $sub i32)
            (local.set $sub (call $started (call $park-confirm (i32.const 0))))
            (if (i32.ne (call $cancel-async (local.get $sub))
                        (i32.const 4 (; CANCELLED_BEFORE_RETURNED ;)))
              (then unreachable))
            (call $cancel-async (local.get $sub)))

          (func (export "confirm-twice") (result i32)
            (call $cancel-async (call $started (call $park-confirm-twice (i32.const 0)))))

          (func (export "return-then-confirm") (result i32)
            (call $cancel-async (call $started (call $park-return-then-confirm (i32.const 0)))))

          (func (export "confirm-uncancelled") (result i32)
            (call $confirm-at-once (i32.const 0))))
        (canon subtask.cancel async (core func $cancel-async))
        (canon subtask.cancel (core func $cancel-sync))
        (canon subtask.drop (core func $subtask.drop))
        (canon waitable.join (core func $waitable.join))
        (canon waitable-set.new (core func $waitable-set.new))
        (canon waitable-set.wait (memory (core memory $memory "mem")) (core func $waitable-set.wait))
        (canon lower (func $park-confirm) async (memory (core memory $memory "mem"))
          (core func $park-confirm'))
        (canon lower (func $park-return) async (memory (core memory $memory "mem"))
          (core func $park-return'))
        (canon lower (func $park-ignore) async (memory (core memory $memory "mem"))
          (core func $park-ignore'))
        (canon lower (func $park-confirm-twice) async (memory (core memory $memory "mem"))
          (core func $park-confirm-twice'))
        (canon lower (func $park-return-then-confirm) async (memory (core memory $memory "mem"))
          (core func $park-return-then-confirm'))
        (canon lower (func $confirm-at-once) async (memory (core memory $memory "mem"))
          (core func $confirm-at-once'))
        (canon lower (func $quick) async (memory (core memory $memory "mem"))
          (core func $quick'))
        (canon lower (func $stackful) async (memory (core memory $memory "mem"))
          (core func $stackful'))
        (canon lower (func $set-backpressure) (core func $set-backpressure'))
        (canon lower (func $clear-backpressure) (core func $clear-backpressure'))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "cancel-async" (func $cancel-async))
          (export "cancel-sync" (func $cancel-sync))
          (export "subtask.drop" (func $subtask.drop))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "park-confirm" (func $park-confirm'))
          (export "park-return" (func $park-return'))
          (export "park-ignore" (func $park-ignore'))
          (export "park-confirm-twice" (func $park-confirm-twice'))
          (export "park-return-then-confirm" (func $park-return-then-confirm'))
          (export "confirm-at-once" (func $confirm-at-once'))
          (export "quick" (func $quick'))
          (export "stackful" (func $stackful'))
          (export "set-backpressure" (func $set-backpressure'))
          (export "clear-backpressure" (func $clear-backpressure'))))))
        (func (export "cancel-at-gate") async (result u32)
          (canon lift (core func $dm "cancel-at-gate")))
        (func (export "cancel-confirming") async (result u32)
          (canon lift (core func $dm "cancel-confirming")))
        (func (export "cancel-returning") async (result u32)
          (canon lift (core func $dm "cancel-returning")))
        (func (export "cancel-stackful-async") async (result u32)
          (canon lift (core func $dm "cancel-stackful-async")))
        (func (export "cancel-stackful-sync") async (result u32)
          (canon lift (core func $dm "cancel-stackful-sync")))
        (func (export "cancel-twice") async (result u32)
          (canon lift (core func $dm "cancel-twice")))
        (func (export "cancel-after-terminal") async (result u32)
          (canon lift (core func $dm "cancel-after-terminal")))
        (func (export "confirm-twice") async (result u32)
          (canon lift (core func $dm "confirm-twice")))
        (func (export "return-then-confirm") async (result u32)
          (canon lift (core func $dm "return-then-confirm")))
        (func (export "confirm-uncancelled") async (result u32)
          (canon lift (core func $dm "confirm-uncancelled"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D
        (with "park-confirm" (func $c "park-confirm"))
        (with "park-return" (func $c "park-return"))
        (with "park-ignore" (func $c "park-ignore"))
        (with "park-confirm-twice" (func $c "park-confirm-twice"))
        (with "park-return-then-confirm" (func $c "park-return-then-confirm"))
        (with "confirm-at-once" (func $c "confirm-at-once"))
        (with "quick" (func $c "quick"))
        (with "stackful" (func $c "stackful"))
        (with "set-backpressure" (func $c "set-backpressure"))
        (with "clear-backpressure" (func $c "clear-backpressure"))))
      (func (export "cancel-at-gate") (alias export $d "cancel-at-gate"))
      (func (export "cancel-confirming") (alias export $d "cancel-confirming"))
      (func (export "cancel-returning") (alias export $d "cancel-returning"))
      (func (export "cancel-stackful-async") (alias export $d "cancel-stackful-async"))
      (func (export "cancel-stackful-sync") (alias export $d "cancel-stackful-sync"))
      (func (export "cancel-twice") (alias export $d "cancel-twice"))
      (func (export "cancel-after-terminal") (alias export $d "cancel-after-terminal"))
      (func (export "confirm-twice") (alias export $d "confirm-twice"))
      (func (export "return-then-confirm") (alias export $d "return-then-confirm"))
      (func (export "confirm-uncancelled") (alias export $d "confirm-uncancelled"))
      (func (export "sync-confirm") (alias export $c "sync-confirm")))
    "#
);

/// A callee whose waitable set already holds an event when a
/// cancellation request reaches it, and a caller that cancels it.
///
/// The callee's core function makes a future, starts a write on its
/// writable end, joins that end to a set, and completes the write with
/// a read of its own readable end. The set now holds the write's
/// event, so the wait word it returns queues the callback at once.
/// The caller cancels before that callback runs. The callback traps
/// unless its first event is the task-cancelled event and its second,
/// after it waits on the set again, is the write's event, which the
/// set kept. It then confirms.
///
/// The caller answers what the cancel answered, or, when that was
/// `BLOCKED`, the state the subtask event later delivers.
const EVENT_QUEUED: &[u8] = component!(
    r#"
    (component
      (component $C
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (type $f (future u32))
        (core func $future.new (canon future.new $f))
        (core func $future.read
          (canon future.read $f async (memory (core memory $memory "mem"))))
        (core func $future.write
          (canon future.write $f async (memory (core memory $memory "mem"))))
        (core func $task.cancel (canon task.cancel))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable.join (canon waitable.join))
        (core module $CM
          (import "" "mem" (memory 1))
          (import "" "future.new" (func $future.new (result i64)))
          (import "" "future.read" (func $future.read (param i32 i32) (result i32)))
          (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
          (import "" "task.cancel" (func $task.cancel))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (global $ws (mut i32) (i32.const 0))
          (global $writable (mut i32) (i32.const 0))
          (global $woken (mut i32) (i32.const 0))
          (func $wait (result i32)
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $ws) (i32.const 4))))
          (func (export "fill-then-wait") (result i32)
            (local $ends i64)
            (local.set $ends (call $future.new))
            (global.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
            (global.set $ws (call $waitable-set.new))
            (if (i32.ne (call $future.write (global.get $writable) (i32.const 100))
                        (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (call $waitable.join (global.get $writable) (global.get $ws))
            (if (i32.ne (call $future.read (i32.wrap_i64 (local.get $ends)) (i32.const 104))
                        (i32.const 0 (; COMPLETED ;)))
              (then unreachable))
            (call $wait))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.eqz (global.get $woken))
              (then
                (if (i32.ne (local.get 0) (i32.const 6 (; TASK_CANCELLED ;)))
                  (then unreachable))
                (global.set $woken (i32.const 1))
                (return (call $wait))))
            (if (i32.ne (local.get 0) (i32.const 5 (; FUTURE_WRITE ;)))
              (then unreachable))
            (if (i32.ne (local.get 1) (global.get $writable))
              (then unreachable))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;))))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "future.new" (func $future.new))
          (export "future.read" (func $future.read))
          (export "future.write" (func $future.write))
          (export "task.cancel" (func $task.cancel))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable.join" (func $waitable.join))))))
        (func (export "fill-then-wait") async (result u32)
          (canon lift (core func $cm "fill-then-wait") async (callback (core func $cm "cb")))))

      (component $D
        (import "fill-then-wait" (func $fill-then-wait async (result u32)))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $cancel-async (canon subtask.cancel async))
        (core func $waitable.join (canon waitable.join))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.wait
          (canon waitable-set.wait (memory (core memory $memory "mem"))))
        (core func $fill-then-wait'
          (canon lower (func $fill-then-wait) async (memory (core memory $memory "mem"))))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "fill-then-wait" (func $fill-then-wait (param i32) (result i32)))
          (func (export "cancel-with-an-event-queued") (result i32)
            (local $status i32) (local $sub i32) (local $ws i32)
            (local.set $status (call $fill-then-wait (i32.const 0)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (local.set $sub (i32.shr_u (local.get $status) (i32.const 4)))
            (local.set $status (call $cancel-async (local.get $sub)))
            (if (i32.eq (local.get $status) (i32.const -1 (; BLOCKED ;)))
              (then
                (local.set $ws (call $waitable-set.new))
                (call $waitable.join (local.get $sub) (local.get $ws))
                (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                            (i32.const 1 (; SUBTASK ;)))
                  (then unreachable))
                (local.set $status (i32.load (i32.const 12)))))
            (local.get $status)))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "cancel-async" (func $cancel-async))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "fill-then-wait" (func $fill-then-wait'))))))
        (func (export "cancel-with-an-event-queued") async (result u32)
          (canon lift (core func $dm "cancel-with-an-event-queued"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D (with "fill-then-wait" (func $c "fill-then-wait"))))
      (func (export "cancel-with-an-event-queued")
        (alias export $d "cancel-with-an-event-queued")))
    "#
);

/// A callee whose callback is queued to take an event its set holds,
/// and a caller that drops the set before the callback runs.
///
/// `$C`'s `fill-then-wait` fills its set with a future write's event,
/// as [`EVENT_QUEUED`]'s callee does, and returns the wait word, which
/// queues the callback at once. The callback checks that its event is
/// the write's, takes the writable end out of the set, drops the set,
/// and returns `7`. `$C`'s synchronous `empty-and-drop` does the same
/// removal and drop from outside the callback.
///
/// `drop-under-the-queued-callback` in `$D` starts `fill-then-wait`
/// and calls `empty-and-drop` while the callback is still queued. The
/// set counts the callback's task as a waiter until the callback takes
/// its event, so the drop traps. `wait-then-drop` starts the same call
/// and only waits for it, so the callback's own drop runs after the
/// callback took its event and succeeds.
const SET_DROPPED: &[u8] = component!(
    r#"
    (component
      (component $C
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (type $f (future u32))
        (core func $future.new (canon future.new $f))
        (core func $future.read
          (canon future.read $f async (memory (core memory $memory "mem"))))
        (core func $future.write
          (canon future.write $f async (memory (core memory $memory "mem"))))
        (core func $task.return (canon task.return (result u32)))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.drop (canon waitable-set.drop))
        (core func $waitable.join (canon waitable.join))
        (core module $CM
          (import "" "mem" (memory 1))
          (import "" "future.new" (func $future.new (result i64)))
          (import "" "future.read" (func $future.read (param i32 i32) (result i32)))
          (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
          (import "" "task.return" (func $task.return (param i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.drop" (func $waitable-set.drop (param i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (global $ws (mut i32) (i32.const 0))
          (global $writable (mut i32) (i32.const 0))
          (func $empty-and-drop
            (call $waitable.join (global.get $writable) (i32.const 0))
            (call $waitable-set.drop (global.get $ws)))
          (func (export "fill-then-wait") (result i32)
            (local $ends i64)
            (local.set $ends (call $future.new))
            (global.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
            (global.set $ws (call $waitable-set.new))
            (if (i32.ne (call $future.write (global.get $writable) (i32.const 100))
                        (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (call $waitable.join (global.get $writable) (global.get $ws))
            (if (i32.ne (call $future.read (i32.wrap_i64 (local.get $ends)) (i32.const 104))
                        (i32.const 0 (; COMPLETED ;)))
              (then unreachable))
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $ws) (i32.const 4))))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 5 (; FUTURE_WRITE ;)))
              (then unreachable))
            (if (i32.ne (local.get 1) (global.get $writable))
              (then unreachable))
            (call $empty-and-drop)
            (call $task.return (i32.const 7))
            (i32.const 0 (; EXIT ;)))
          (func (export "empty-and-drop") (call $empty-and-drop)))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "future.new" (func $future.new))
          (export "future.read" (func $future.read))
          (export "future.write" (func $future.write))
          (export "task.return" (func $task.return))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.drop" (func $waitable-set.drop))
          (export "waitable.join" (func $waitable.join))))))
        (func (export "fill-then-wait") async (result u32)
          (canon lift (core func $cm "fill-then-wait") async (callback (core func $cm "cb"))))
        (func (export "empty-and-drop")
          (canon lift (core func $cm "empty-and-drop"))))

      (component $D
        (import "fill-then-wait" (func $fill-then-wait async (result u32)))
        (import "empty-and-drop" (func $empty-and-drop))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $waitable.join (canon waitable.join))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.wait
          (canon waitable-set.wait (memory (core memory $memory "mem"))))
        (core func $fill-then-wait'
          (canon lower (func $fill-then-wait) async (memory (core memory $memory "mem"))))
        (core func $empty-and-drop' (canon lower (func $empty-and-drop)))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "fill-then-wait" (func $fill-then-wait (param i32) (result i32)))
          (import "" "empty-and-drop" (func $empty-and-drop))
          (func $start (result i32)
            (local $status i32)
            (local.set $status (call $fill-then-wait (i32.const 0)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (i32.shr_u (local.get $status) (i32.const 4)))
          (func $finish (param $sub i32) (result i32)
            (local $ws i32)
            (local.set $ws (call $waitable-set.new))
            (call $waitable.join (local.get $sub) (local.get $ws))
            (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                        (i32.const 1 (; SUBTASK ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 12)) (i32.const 2 (; RETURNED ;)))
              (then unreachable))
            (i32.load (i32.const 0)))
          (func (export "drop-under-the-queued-callback") (result i32)
            (local $sub i32)
            (local.set $sub (call $start))
            (call $empty-and-drop)
            (call $finish (local.get $sub)))
          (func (export "wait-then-drop") (result i32)
            (call $finish (call $start))))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "fill-then-wait" (func $fill-then-wait'))
          (export "empty-and-drop" (func $empty-and-drop'))))))
        (func (export "drop-under-the-queued-callback") async (result u32)
          (canon lift (core func $dm "drop-under-the-queued-callback")))
        (func (export "wait-then-drop") async (result u32)
          (canon lift (core func $dm "wait-then-drop"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D
        (with "fill-then-wait" (func $c "fill-then-wait"))
        (with "empty-and-drop" (func $c "empty-and-drop"))))
      (func (export "drop-under-the-queued-callback")
        (alias export $d "drop-under-the-queued-callback"))
      (func (export "wait-then-drop") (alias export $d "wait-then-drop")))
    "#
);

/// A callee whose callback, woken by the request, blocks before it
/// confirms, and a caller that cancels it.
///
/// The caller makes a future and passes its readable end to `$C`'s
/// `park-then-read`. On the task-cancelled event, that callback reads
/// the future synchronously, and only the caller writes it, after the
/// cancel returns. The cancel gives way to the woken callback from
/// inside its own frame, so the callback suspends above the cancel,
/// and the cancel goes on without it, answering `BLOCKED`.
///
/// `park-then-trap`'s callback traps on the event instead. In the
/// browser the JSPI provider hands that failure over on a microtask,
/// so the cancel leaves the callee's run to the store and fails once
/// the store has taken the failure.
///
/// The caller then writes the future, and answers what the cancel
/// answered, or, when that was `BLOCKED`, the state the subtask event
/// later delivers.
const SUSPENDS: &[u8] = component!(
    r#"
    (component
      (component $C
        (type $f (future u32))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $task.cancel (canon task.cancel))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $future.read (canon future.read $f (memory (core memory $memory "mem"))))
        (core module $CM
          (import "" "mem" (memory 1))
          (import "" "task.cancel" (func $task.cancel))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "future.read" (func $future.read (param i32 i32) (result i32)))
          (global $ws (mut i32) (i32.const 0))
          (global $readable (mut i32) (i32.const 0))
          (func (export "park") (param i32) (result i32)
            (global.set $readable (local.get 0))
            (global.set $ws (call $waitable-set.new))
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $ws) (i32.const 4))))
          (func (export "trap-cb") (param i32 i32 i32) (result i32)
            unreachable)
          (func (export "read-then-confirm-cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 6 (; TASK_CANCELLED ;)))
              (then unreachable))
            (if (i32.ne (call $future.read (global.get $readable) (i32.const 100))
                        (i32.const 0 (; COMPLETED ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 100)) (i32.const 42))
              (then unreachable))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;))))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "task.cancel" (func $task.cancel))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "future.read" (func $future.read))))))
        (func (export "park-then-read") async (param "f" $f) (result u32)
          (canon lift (core func $cm "park") async
            (callback (core func $cm "read-then-confirm-cb"))))
        (func (export "park-then-trap") async (param "f" $f) (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "trap-cb")))))

      (component $D
        (type $f (future u32))
        (import "park-then-read" (func $park-then-read async (param "f" $f) (result u32)))
        (import "park-then-trap" (func $park-then-trap async (param "f" $f) (result u32)))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $future.new (canon future.new $f))
        (core func $future.write
          (canon future.write $f async (memory (core memory $memory "mem"))))
        (core func $cancel-async (canon subtask.cancel async))
        (core func $waitable.join (canon waitable.join))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.wait
          (canon waitable-set.wait (memory (core memory $memory "mem"))))
        (core func $park-then-read'
          (canon lower (func $park-then-read) async (memory (core memory $memory "mem"))))
        (core func $park-then-trap'
          (canon lower (func $park-then-trap) async (memory (core memory $memory "mem"))))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "future.new" (func $future.new (result i64)))
          (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "park-then-read" (func $park-then-read (param i32 i32) (result i32)))
          (import "" "park-then-trap" (func $park-then-trap (param i32 i32) (result i32)))
          (func (export "cancel-a-callee-that-traps") (result i32)
            (local $status i32)
            (local.set $status
              (call $park-then-trap (i32.wrap_i64 (call $future.new)) (i32.const 0)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (call $cancel-async (i32.shr_u (local.get $status) (i32.const 4))))
          (func (export "cancel-a-callee-that-blocks") (result i32)
            (local $ends i64) (local $status i32) (local $sub i32) (local $ws i32)
            (local.set $ends (call $future.new))
            (local.set $status
              (call $park-then-read (i32.wrap_i64 (local.get $ends)) (i32.const 0)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (local.set $sub (i32.shr_u (local.get $status) (i32.const 4)))
            (local.set $status (call $cancel-async (local.get $sub)))
            (i32.store (i32.const 200) (i32.const 42))
            (drop (call $future.write
              (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32)))
              (i32.const 200)))
            (if (i32.eq (local.get $status) (i32.const -1 (; BLOCKED ;)))
              (then
                (local.set $ws (call $waitable-set.new))
                (call $waitable.join (local.get $sub) (local.get $ws))
                (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                            (i32.const 1 (; SUBTASK ;)))
                  (then unreachable))
                (local.set $status (i32.load (i32.const 12)))))
            (local.get $status)))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "future.new" (func $future.new))
          (export "future.write" (func $future.write))
          (export "cancel-async" (func $cancel-async))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "park-then-read" (func $park-then-read'))
          (export "park-then-trap" (func $park-then-trap'))))))
        (func (export "cancel-a-callee-that-blocks") async (result u32)
          (canon lift (core func $dm "cancel-a-callee-that-blocks")))
        (func (export "cancel-a-callee-that-traps") async (result u32)
          (canon lift (core func $dm "cancel-a-callee-that-traps"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D
        (with "park-then-read" (func $c "park-then-read"))
        (with "park-then-trap" (func $c "park-then-trap"))))
      (func (export "cancel-a-callee-that-blocks")
        (alias export $d "cancel-a-callee-that-blocks"))
      (func (export "cancel-a-callee-that-traps")
        (alias export $d "cancel-a-callee-that-traps")))
    "#
);

/// A callee whose callback, woken by the request, switches to a
/// suspended thread of its own task before it confirms, and a caller
/// that cancels it.
///
/// `$C`'s `park` starts a helper thread with `thread.yield-then-resume`
/// and parks in its loop once the helper has suspended. On the
/// task-cancelled event the callback switches to the helper with
/// `thread.suspend-then-resume`. The helper marks that it ran, makes
/// the callback's thread ready with `thread.resume-later`, and ends.
/// The callback then traps unless the helper ran, and confirms.
///
/// The caller answers what the cancel answered, or, when that was
/// `BLOCKED`, the state the subtask event later delivers.
const SWITCHES: &[u8] = component!(
    r#"
    (component
      (component $C
        (core module $Libc
          (memory (export "mem") 1)
          (table (export "table") 1 funcref))
        (core instance $libc (instantiate $Libc))
        (core type $start-ty (func (param i32)))
        (alias core export $libc "table" (core table $table))
        (core func $task.cancel (canon task.cancel))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $thread.index (canon thread.index))
        (core func $thread.new-indirect (canon thread.new-indirect $start-ty (core table $table)))
        (core func $thread.suspend (canon thread.suspend))
        (core func $thread.resume-later (canon thread.resume-later))
        (core func $thread.suspend-then-resume (canon thread.suspend-then-resume))
        (core func $thread.yield-then-resume (canon thread.yield-then-resume))
        (core module $CM
          (import "" "task.cancel" (func $task.cancel))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "thread.index" (func $thread.index (result i32)))
          (import "" "thread.new-indirect" (func $thread.new-indirect (param i32 i32) (result i32)))
          (import "" "thread.suspend" (func $thread.suspend (result i32)))
          (import "" "thread.resume-later" (func $thread.resume-later (param i32)))
          (import "" "thread.suspend-then-resume"
            (func $thread.suspend-then-resume (param i32) (result i32)))
          (import "" "thread.yield-then-resume"
            (func $thread.yield-then-resume (param i32) (result i32)))
          (import "" "table" (table 1 funcref))
          (global $main (mut i32) (i32.const 0))
          (global $helper (mut i32) (i32.const 0))
          (global $helped (mut i32) (i32.const 0))
          (func $help (param i32)
            (drop (call $thread.suspend))
            (global.set $helped (i32.const 1))
            (call $thread.resume-later (global.get $main)))
          (elem (table 0) (i32.const 0) func $help)
          (func (export "park") (result i32)
            (global.set $main (call $thread.index))
            (global.set $helper (call $thread.new-indirect (i32.const 0) (i32.const 0)))
            (drop (call $thread.yield-then-resume (global.get $helper)))
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (call $waitable-set.new) (i32.const 4))))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 6 (; TASK_CANCELLED ;)))
              (then unreachable))
            (drop (call $thread.suspend-then-resume (global.get $helper)))
            (if (i32.eqz (global.get $helped))
              (then unreachable))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;))))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "task.cancel" (func $task.cancel))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "thread.index" (func $thread.index))
          (export "thread.new-indirect" (func $thread.new-indirect))
          (export "thread.suspend" (func $thread.suspend))
          (export "thread.resume-later" (func $thread.resume-later))
          (export "thread.suspend-then-resume" (func $thread.suspend-then-resume))
          (export "thread.yield-then-resume" (func $thread.yield-then-resume))
          (export "table" (table $table))))))
        (func (export "park-then-switch") async (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "cb")))))

      (component $D
        (import "park-then-switch" (func $park-then-switch async (result u32)))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $cancel-async (canon subtask.cancel async))
        (core func $waitable.join (canon waitable.join))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.wait
          (canon waitable-set.wait (memory (core memory $memory "mem"))))
        (core func $park-then-switch'
          (canon lower (func $park-then-switch) async (memory (core memory $memory "mem"))))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "park-then-switch" (func $park-then-switch (param i32) (result i32)))
          (func (export "cancel-a-callee-that-switches") (result i32)
            (local $status i32) (local $sub i32) (local $ws i32)
            (local.set $status (call $park-then-switch (i32.const 0)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (local.set $sub (i32.shr_u (local.get $status) (i32.const 4)))
            (local.set $status (call $cancel-async (local.get $sub)))
            (if (i32.eq (local.get $status) (i32.const -1 (; BLOCKED ;)))
              (then
                (local.set $ws (call $waitable-set.new))
                (call $waitable.join (local.get $sub) (local.get $ws))
                (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                            (i32.const 1 (; SUBTASK ;)))
                  (then unreachable))
                (local.set $status (i32.load (i32.const 12)))))
            (local.get $status)))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "cancel-async" (func $cancel-async))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "park-then-switch" (func $park-then-switch'))))))
        (func (export "cancel-a-callee-that-switches") async (result u32)
          (canon lift (core func $dm "cancel-a-callee-that-switches"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D (with "park-then-switch" (func $c "park-then-switch"))))
      (func (export "cancel-a-callee-that-switches")
        (alias export $d "cancel-a-callee-that-switches")))
    "#
);

/// A caller that lends a callee a borrow of a resource neither of
/// them implements, and cancels the call.
///
/// `$R` implements the resource and mints its handles. Each export of
/// `$D` mints an owning handle, calls one export of `$C` through an
/// asynchronous lower with a borrow of it, cancels the call, and then
/// drops the owning handle before it drops the subtask. The drop traps
/// while the handle is lent, so an export that answers says the
/// cancel's delivery of the resolution gave the lend back.
///
/// Every export of `$C` parks in its loop on a set no turn fills and
/// keeps the borrow's index. On the task-cancelled event
/// `park-confirm` drops the borrow and confirms, `park-return` drops
/// it and returns `7`, `park-confirm-borrowing` confirms without
/// dropping it, and `park-ignore` parks again, borrow and all.
/// `drop-while-lent` cancels a callee that ignores the request, so the
/// call is unresolved and the handle still lent when it drops it.
/// `drop-while-gated` drops the handle before it cancels a call the
/// entry gate holds. A call lifts its arguments, and so lends the
/// borrow, only as its callee starts, which is where the reference's
/// `on_start` lifts them, so the gated call has lent nothing and the
/// drop goes through.
const LENDS: &[u8] = component!(
    r#"
    (component
      (component $R
        (type $t' (resource (rep i32)))
        (core func $new (canon resource.new $t'))
        (core module $RM
          (import "" "new" (func $new (param i32) (result i32)))
          (func (export "make") (param i32) (result i32) (call $new (local.get 0))))
        (core instance $rm (instantiate $RM (with "" (instance (export "new" (func $new))))))
        (export $t "thing" (type $t'))
        (func (export "make") (param "rep" u32) (result (own $t))
          (canon lift (core func $rm "make"))))

      (component $C
        (import "thing" (type $t (sub resource)))
        (core func $task.cancel (canon task.cancel))
        (core func $task.return (canon task.return (result u32)))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $drop-borrow (canon resource.drop $t))
        (core func $backpressure.inc (canon backpressure.inc))
        (core func $backpressure.dec (canon backpressure.dec))
        (core module $CM
          (import "" "task.cancel" (func $task.cancel))
          (import "" "task.return" (func $task.return (param i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "drop-borrow" (func $drop-borrow (param i32)))
          (import "" "backpressure.inc" (func $backpressure.inc))
          (import "" "backpressure.dec" (func $backpressure.dec))
          (global $ws (mut i32) (i32.const 0))
          (global $borrow (mut i32) (i32.const 0))
          (func $start (global.set $ws (call $waitable-set.new)))
          (start $start)
          (func $wait (result i32)
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (global.get $ws) (i32.const 4))))
          (func $expect-cancelled (param $event i32)
            (if (i32.ne (local.get $event) (i32.const 6 (; TASK_CANCELLED ;)))
              (then unreachable)))
          (func (export "park") (param i32) (result i32)
            (global.set $borrow (local.get 0))
            (call $wait))
          (func (export "confirm-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $drop-borrow (global.get $borrow))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "return-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $drop-borrow (global.get $borrow))
            (call $task.return (i32.const 7))
            (i32.const 0 (; EXIT ;)))
          (func (export "confirm-borrowing-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))
          (func (export "ignore-cb") (param i32 i32 i32) (result i32)
            (call $expect-cancelled (local.get 0))
            (call $wait))
          (func (export "set-backpressure") (call $backpressure.inc))
          (func (export "clear-backpressure") (call $backpressure.dec)))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "task.cancel" (func $task.cancel))
          (export "task.return" (func $task.return))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "drop-borrow" (func $drop-borrow))
          (export "backpressure.inc" (func $backpressure.inc))
          (export "backpressure.dec" (func $backpressure.dec))))))
        (func (export "park-confirm") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "confirm-cb"))))
        (func (export "park-return") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "return-cb"))))
        (func (export "park-confirm-borrowing") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $cm "park") async
            (callback (core func $cm "confirm-borrowing-cb"))))
        (func (export "park-ignore") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $cm "park") async (callback (core func $cm "ignore-cb"))))
        (func (export "set-backpressure") (canon lift (core func $cm "set-backpressure")))
        (func (export "clear-backpressure") (canon lift (core func $cm "clear-backpressure"))))

      (component $D
        (import "thing" (type $t (sub resource)))
        (import "make" (func $make (param "rep" u32) (result (own $t))))
        (import "park-confirm" (func $park-confirm async (param "x" (borrow $t)) (result u32)))
        (import "park-return" (func $park-return async (param "x" (borrow $t)) (result u32)))
        (import "park-confirm-borrowing"
          (func $park-confirm-borrowing async (param "x" (borrow $t)) (result u32)))
        (import "park-ignore" (func $park-ignore async (param "x" (borrow $t)) (result u32)))
        (import "set-backpressure" (func $set-backpressure))
        (import "clear-backpressure" (func $clear-backpressure))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (core func $make' (canon lower (func $make)))
        (core func $drop-thing (canon resource.drop $t))
        (core func $cancel-async (canon subtask.cancel async))
        (core func $subtask.drop (canon subtask.drop))
        (core func $park-confirm'
          (canon lower (func $park-confirm) async (memory (core memory $memory "mem"))))
        (core func $park-return'
          (canon lower (func $park-return) async (memory (core memory $memory "mem"))))
        (core func $park-confirm-borrowing'
          (canon lower (func $park-confirm-borrowing) async (memory (core memory $memory "mem"))))
        (core func $park-ignore'
          (canon lower (func $park-ignore) async (memory (core memory $memory "mem"))))
        (core func $set-backpressure' (canon lower (func $set-backpressure)))
        (core func $clear-backpressure' (canon lower (func $clear-backpressure)))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "make" (func $make (param i32) (result i32)))
          (import "" "drop-thing" (func $drop-thing (param i32)))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "subtask.drop" (func $subtask.drop (param i32)))
          (import "" "park-confirm" (func $park-confirm (param i32 i32) (result i32)))
          (import "" "park-return" (func $park-return (param i32 i32) (result i32)))
          (import "" "park-confirm-borrowing"
            (func $park-confirm-borrowing (param i32 i32) (result i32)))
          (import "" "park-ignore" (func $park-ignore (param i32 i32) (result i32)))
          (import "" "set-backpressure" (func $set-backpressure))
          (import "" "clear-backpressure" (func $clear-backpressure))

          ;; The subtask index a lower's status word carries, once the
          ;; state in its low bits is the one expected.
          (func $subtask (param $status i32) (param $state i32) (result i32)
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (local.get $state))
              (then unreachable))
            (i32.shr_u (local.get $status) (i32.const 4)))

          ;; Cancel the call `sub` records, drop the owning handle
          ;; `handle` it borrowed, then drop the subtask, and answer
          ;; what the cancel answered.
          (func $cancel-then-drop (param $sub i32) (param $handle i32) (result i32)
            (local $status i32)
            (local.set $status (call $cancel-async (local.get $sub)))
            (call $drop-thing (local.get $handle))
            (call $subtask.drop (local.get $sub))
            (local.get $status))

          (func (export "drop-while-gated") (result i32)
            (local $handle i32) (local $sub i32) (local $status i32)
            (local.set $handle (call $make (i32.const 1)))
            (call $set-backpressure)
            (local.set $sub
              (call $subtask (call $park-confirm (local.get $handle) (i32.const 0))
                             (i32.const 0 (; STARTING ;))))
            (call $drop-thing (local.get $handle))
            (local.set $status (call $cancel-async (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (call $clear-backpressure)
            (local.get $status))


          (func (export "cancel-confirming") (result i32)
            (local $handle i32)
            (local.set $handle (call $make (i32.const 1)))
            (call $cancel-then-drop
              (call $subtask (call $park-confirm (local.get $handle) (i32.const 0))
                             (i32.const 1 (; STARTED ;)))
              (local.get $handle)))

          (func (export "cancel-returning") (result i32)
            (local $handle i32)
            (local.set $handle (call $make (i32.const 1)))
            (call $cancel-then-drop
              (call $subtask (call $park-return (local.get $handle) (i32.const 0))
                             (i32.const 1 (; STARTED ;)))
              (local.get $handle)))

          (func (export "confirm-borrowing") (result i32)
            (local $handle i32)
            (local.set $handle (call $make (i32.const 1)))
            (call $cancel-async
              (call $subtask (call $park-confirm-borrowing (local.get $handle) (i32.const 0))
                             (i32.const 1 (; STARTED ;)))))

          (func (export "drop-while-lent") (result i32)
            (local $handle i32)
            (local.set $handle (call $make (i32.const 1)))
            (if (i32.ne
                  (call $cancel-async
                    (call $subtask (call $park-ignore (local.get $handle) (i32.const 0))
                                   (i32.const 1 (; STARTED ;))))
                  (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (call $drop-thing (local.get $handle))
            (i32.const 0)))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "make" (func $make'))
          (export "drop-thing" (func $drop-thing))
          (export "cancel-async" (func $cancel-async))
          (export "subtask.drop" (func $subtask.drop))
          (export "park-confirm" (func $park-confirm'))
          (export "park-return" (func $park-return'))
          (export "park-confirm-borrowing" (func $park-confirm-borrowing'))
          (export "park-ignore" (func $park-ignore'))
          (export "set-backpressure" (func $set-backpressure'))
          (export "clear-backpressure" (func $clear-backpressure'))))))
        (func (export "drop-while-gated") async (result u32)
          (canon lift (core func $dm "drop-while-gated")))
        (func (export "cancel-confirming") async (result u32)
          (canon lift (core func $dm "cancel-confirming")))
        (func (export "cancel-returning") async (result u32)
          (canon lift (core func $dm "cancel-returning")))
        (func (export "confirm-borrowing") async (result u32)
          (canon lift (core func $dm "confirm-borrowing")))
        (func (export "drop-while-lent") async (result u32)
          (canon lift (core func $dm "drop-while-lent"))))

      (instance $r (instantiate $R))
      (alias export $r "thing" (type $t))
      (instance $c (instantiate $C (with "thing" (type $t))))
      (instance $d (instantiate $D
        (with "thing" (type $t))
        (with "make" (func $r "make"))
        (with "park-confirm" (func $c "park-confirm"))
        (with "park-return" (func $c "park-return"))
        (with "park-confirm-borrowing" (func $c "park-confirm-borrowing"))
        (with "park-ignore" (func $c "park-ignore"))
        (with "set-backpressure" (func $c "set-backpressure"))
        (with "clear-backpressure" (func $c "clear-backpressure"))))
      (func (export "drop-while-gated") (alias export $d "drop-while-gated"))
      (func (export "cancel-confirming") (alias export $d "cancel-confirming"))
      (func (export "cancel-returning") (alias export $d "cancel-returning"))
      (func (export "confirm-borrowing") (alias export $d "confirm-borrowing"))
      (func (export "drop-while-lent") (alias export $d "drop-while-lent")))
    "#
);

/// A component that calls three host `async` functions and cancels
/// the calls.
///
/// `never` stays pending for ever, `echo` is pending once and then
/// answers its argument, and `dropped` is synchronous and answers how
/// many futures of `never` the host has seen dropped. Each export
/// writes what it saw at address 64 and answers that address, which
/// the lift reads as a tuple:
///
/// - `cancel-async` cancels a call of `never` asynchronously, asks
///   `dropped` at once, and then waits for the subtask event, whose
///   state it keeps.
/// - `cancel-sync` cancels a call of `never` synchronously, and asks
///   `dropped` once the cancel returned.
/// - `cancel-returned` starts two calls of `echo`, waits for the
///   second one's event, and then cancels the first, whose future
///   completed in the same poll as the second's. It keeps the state
///   the wait carried, what the cancel answered, and what the first
///   call left at its result address.
const HOST_CALLS: &[u8] = component!(
    r#"
    (component
      (import "never" (func $never async (result u32)))
      (import "echo" (func $echo async (param "v" u32) (result u32)))
      (import "dropped" (func $dropped (result u32)))
      (core module $Memory (memory (export "mem") 1))
      (core instance $memory (instantiate $Memory))
      (core func $never' (canon lower (func $never) async (memory (core memory $memory "mem"))))
      (core func $echo' (canon lower (func $echo) async (memory (core memory $memory "mem"))))
      (core func $dropped' (canon lower (func $dropped)))
      (core func $cancel-sync (canon subtask.cancel))
      (core func $cancel-async (canon subtask.cancel async))
      (core func $subtask.drop (canon subtask.drop))
      (core func $waitable-set.new (canon waitable-set.new))
      (core func $waitable.join (canon waitable.join))
      (core func $waitable-set.wait
        (canon waitable-set.wait (memory (core memory $memory "mem"))))
      (core func $waitable-set.drop (canon waitable-set.drop))
      (core module $M
        (import "" "mem" (memory 1))
        (import "" "never" (func $never (param i32) (result i32)))
        (import "" "echo" (func $echo (param i32 i32) (result i32)))
        (import "" "dropped" (func $dropped (result i32)))
        (import "" "cancel-sync" (func $cancel-sync (param i32) (result i32)))
        (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
        (import "" "subtask.drop" (func $subtask.drop (param i32)))
        (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
        (import "" "waitable.join" (func $waitable.join (param i32 i32)))
        (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
        (import "" "waitable-set.drop" (func $waitable-set.drop (param i32)))

        ;; The subtask index a lower's status word carries, once the
        ;; state in its low bits is STARTED.
        (func $started (param $status i32) (result i32)
          (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
            (then unreachable))
          (i32.shr_u (local.get $status) (i32.const 4)))

        ;; Wait for the subtask event of `sub`, and answer the state it
        ;; carries.
        (func $wait-for (param $sub i32) (result i32)
          (local $ws i32)
          (local.set $ws (call $waitable-set.new))
          (call $waitable.join (local.get $sub) (local.get $ws))
          (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 0))
                      (i32.const 1 (; SUBTASK ;)))
            (then unreachable))
          (if (i32.ne (i32.load (i32.const 0)) (local.get $sub))
            (then unreachable))
          (call $waitable.join (local.get $sub) (i32.const 0))
          (call $waitable-set.drop (local.get $ws))
          (i32.load (i32.const 4)))

        (func (export "cancel-async") (result i32)
          (local $sub i32)
          (local.set $sub (call $started (call $never (i32.const 16))))
          (i32.store (i32.const 64) (call $cancel-async (local.get $sub)))
          (i32.store (i32.const 68) (call $dropped))
          (i32.store (i32.const 72) (call $wait-for (local.get $sub)))
          (call $subtask.drop (local.get $sub))
          (i32.const 64))

        (func (export "cancel-sync") (result i32)
          (local $sub i32)
          (local.set $sub (call $started (call $never (i32.const 16))))
          (i32.store (i32.const 64) (call $cancel-sync (local.get $sub)))
          (i32.store (i32.const 68) (call $dropped))
          (call $subtask.drop (local.get $sub))
          (i32.const 64))

        (func (export "cancel-returned") (result i32)
          (local $first i32) (local $second i32)
          (local.set $first (call $started (call $echo (i32.const 7) (i32.const 16))))
          (local.set $second (call $started (call $echo (i32.const 9) (i32.const 20))))
          (i32.store (i32.const 64) (call $wait-for (local.get $second)))
          (i32.store (i32.const 68) (call $cancel-async (local.get $first)))
          (i32.store (i32.const 72) (i32.load (i32.const 16)))
          (call $subtask.drop (local.get $first))
          (call $subtask.drop (local.get $second))
          (i32.const 64)))
      (core instance $i (instantiate $M (with "" (instance
        (export "mem" (memory $memory "mem"))
        (export "never" (func $never'))
        (export "echo" (func $echo'))
        (export "dropped" (func $dropped'))
        (export "cancel-sync" (func $cancel-sync))
        (export "cancel-async" (func $cancel-async))
        (export "subtask.drop" (func $subtask.drop))
        (export "waitable-set.new" (func $waitable-set.new))
        (export "waitable.join" (func $waitable.join))
        (export "waitable-set.wait" (func $waitable-set.wait))
        (export "waitable-set.drop" (func $waitable-set.drop))))))
      (func (export "cancel-async") async (result (tuple u32 u32 u32))
        (canon lift (core func $i "cancel-async") (memory (core memory $memory "mem"))))
      (func (export "cancel-sync") async (result (tuple u32 u32))
        (canon lift (core func $i "cancel-sync") (memory (core memory $memory "mem"))))
      (func (export "cancel-returned") async (result (tuple u32 u32 u32))
        (canon lift (core func $i "cancel-returned") (memory (core memory $memory "mem")))))
    "#
);

/// Instantiate `binary` in a store of its own. The engine accepts the
/// stackful form of `canon lift async`, which the stackful callee is
/// lifted in, the thread built-ins, which a callee that switches
/// threads calls, and the `async` option of `subtask.cancel`: Wasmtime
/// keeps each behind a flag.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let mut config = EngineConfig::new();
    config
        .wasm_component_model_async_stackful(true)
        .wasm_component_model_threading(true)
        .wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, binary)
        .await
        .expect("the component translates");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

async fn call(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> Vec<Val> {
    let func = instance.get_func(name).expect("the export is declared");
    func.call(store, args)
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {}", chain(&error)))
        .into_vec()
}

async fn call_failing(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> Error {
    let func = instance.get_func(name).expect("the export is declared");
    match func.call(store, args).await {
        Err(error) => error,
        Ok(values) => panic!("{name} returned {values:?} rather than failing"),
    }
}

/// Call the caller's export `name` in a fresh instance of [`CALLS`]
/// and answer the `u32` it returned.
async fn cancel(name: &str) -> u32 {
    cancel_in(CALLS, name).await
}

/// Call the export `name` in a fresh instance of `binary` and answer
/// the `u32` it returned.
async fn cancel_in(binary: &[u8], name: &str) -> u32 {
    let (mut store, instance) = instantiate(binary).await;
    match call(&mut store, &instance, name, &[]).await.as_slice() {
        [Val::U32(status)] => *status,
        other => panic!("{name} answered {other:?} rather than one u32"),
    }
}

/// Call the export `name` in a fresh instance of [`CALLS`] and answer
/// the message chain of the failure it must end with.
async fn cancel_failing(name: &str) -> String {
    cancel_failing_in(CALLS, name).await
}

/// Call the export `name` in a fresh instance of `binary` and answer
/// the message chain of the failure it must end with.
async fn cancel_failing_in(binary: &[u8], name: &str) -> String {
    let (mut store, instance) = instantiate(binary).await;
    chain(&call_failing(&mut store, &instance, name, &[]).await)
}

/// Every message in an error's source chain, joined so that a trap
/// is matched wherever the substrate put it.
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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_fails_with(message: &str, expected: &str, what: &str) {
    assert!(
        message.contains(expected),
        "{what} must fail with `{expected}`, got: {message}"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_path_that_does_not_cancel_in_a_component_that_links_both_built_ins() {
    let (mut store, instance) = instantiate(CANCELS).await;

    let results = call(&mut store, &instance, "answer", &[]).await;

    assert!(
        matches!(results.as_slice(), [Val::U32(42)]),
        "the export that never cancels returns its result: {results:?}"
    );
}

#[wcmp_macros::test]
async fn it_checks_the_may_leave_flag_before_anything_else_in_either_built_in() {
    for which in [TASK_CANCEL, SUBTASK_CANCEL] {
        let (mut store, instance) = instantiate(CANCELS).await;
        call(&mut store, &instance, "select", &[Val::U32(which)]).await;

        // The export's result is lifted first, and the `post-return`
        // then runs with the instance's may-leave flag clear.
        let error = call_failing(&mut store, &instance, "run", &[]).await;

        assert_fails_with(
            &chain(&error),
            &TaskCause::CannotLeave.to_string(),
            "a cancellation built-in called from a post-return",
        );
    }
}

#[wcmp_macros::test]
async fn it_runs_a_post_return_that_calls_neither_built_in() {
    let (mut store, instance) = instantiate(CANCELS).await;
    call(&mut store, &instance, "select", &[Val::U32(NEITHER)]).await;

    let results = call(&mut store, &instance, "run", &[]).await;

    assert!(
        matches!(results.as_slice(), [Val::U32(5)]),
        "the export returns once its post-return has run: {results:?}"
    );
}

#[wcmp_macros::test]
async fn it_resolves_a_callee_held_at_the_entry_gate_as_cancelled_before_started() {
    assert_eq!(
        cancel("cancel-at-gate").await,
        CANCELLED_BEFORE_STARTED,
        "a callee the gate holds never runs, and the next call passes the gate"
    );
}

#[wcmp_macros::test]
async fn it_resolves_a_callee_that_confirms_as_cancelled_before_returned() {
    assert_eq!(
        cancel("cancel-confirming").await,
        CANCELLED_BEFORE_RETURNED,
        "a callee waiting in its loop takes the request at once and confirms it"
    );
}

#[wcmp_macros::test]
async fn it_resolves_a_callee_that_returns_anyway_as_returned() {
    assert_eq!(
        cancel("cancel-returning").await,
        RETURNED,
        "a callee told of the request may return its result instead"
    );
}

#[wcmp_macros::test]
async fn it_never_tells_a_stackful_callee_and_answers_blocked_to_an_asynchronous_cancel() {
    assert_eq!(
        cancel("cancel-stackful-async").await,
        RETURNED,
        "the cancel answers `BLOCKED`, and the callee returns on its own"
    );
}

#[wcmp_macros::test]
async fn it_waits_for_a_stackful_callee_to_return_in_a_synchronous_cancel() {
    assert_eq!(
        cancel("cancel-stackful-sync").await,
        RETURNED,
        "a synchronous cancel of a callee that is never told waits for its return"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_second_cancel_of_a_subtask_whose_resolution_is_still_owed() {
    assert_fails_with(
        &cancel_failing("cancel-twice").await,
        &WaitableCause::SubtaskCancelledTwice.to_string(),
        "a second `subtask.cancel` of one subtask",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_cancel_after_the_resolution_was_delivered() {
    assert_fails_with(
        &cancel_failing("cancel-after-terminal").await,
        &WaitableCause::SubtaskCancelAfterTerminal.to_string(),
        "a `subtask.cancel` after the cancel that delivered the resolution",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_task_cancel_in_a_callback_task_that_was_never_cancelled() {
    assert_fails_with(
        &cancel_failing("confirm-uncancelled").await,
        &TaskCause::CancelNotDelivered.to_string(),
        "a `task.cancel` no request was delivered to",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_task_cancel_in_a_task_that_is_not_lifted_async() {
    assert_fails_with(
        &cancel_failing("sync-confirm").await,
        &TaskCause::CancelNotDelivered.to_string(),
        "a `task.cancel` in a synchronously lifted export",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_second_task_cancel_of_a_cancelled_task() {
    assert_fails_with(
        &cancel_failing("confirm-twice").await,
        &TaskCause::ReturnedTwice.to_string(),
        "a second `task.cancel`",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_task_cancel_after_a_cancelled_task_returned() {
    assert_fails_with(
        &cancel_failing("return-then-confirm").await,
        &TaskCause::ReturnedTwice.to_string(),
        "a `task.cancel` after `task.return`",
    );
}

#[wcmp_macros::test]
async fn it_delivers_the_request_before_an_event_the_set_already_holds() {
    assert_eq!(
        cancel_in(EVENT_QUEUED, "cancel-with-an-event-queued").await,
        CANCELLED_BEFORE_RETURNED,
        "the callback receives event 6 first and the write's event at its next wait"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_task_cancel_while_the_task_still_holds_a_borrow() {
    assert_fails_with(
        &cancel_failing_in(LENDS, "confirm-borrowing").await,
        "borrow handles still remain at the end of the call",
        "a `task.cancel` with a borrow outstanding",
    );
}

#[wcmp_macros::test]
async fn it_keeps_a_lent_handle_lent_while_the_resolution_is_owed() {
    assert_fails_with(
        &cancel_failing_in(LENDS, "drop-while-lent").await,
        "cannot remove owned resource while borrowed",
        "a drop of the handle a cancelled but unresolved call borrowed",
    );
}

#[wcmp_macros::test]
async fn it_releases_the_lend_when_a_cancel_delivers_cancelled_before_returned() {
    assert_eq!(
        cancel_in(LENDS, "cancel-confirming").await,
        CANCELLED_BEFORE_RETURNED,
        "the handle the confirmed call borrowed drops once the cancel answers"
    );
}

#[wcmp_macros::test]
async fn it_releases_the_lend_when_a_cancel_delivers_returned() {
    assert_eq!(
        cancel_in(LENDS, "cancel-returning").await,
        RETURNED,
        "the handle the returned call borrowed drops once the cancel answers"
    );
}

#[wcmp_macros::test]
async fn it_finishes_a_cancel_whose_woken_callee_blocks_before_it_confirms() {
    assert_eq!(
        cancel_in(SUSPENDS, "cancel-a-callee-that-blocks").await,
        CANCELLED_BEFORE_RETURNED,
        "the woken callback blocks above the cancel, and confirms once the caller writes"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_cancel_whose_woken_callee_traps() {
    assert_fails_with(
        &cancel_failing_in(SUSPENDS, "cancel-a-callee-that-traps").await,
        "unreachable",
        "a cancel that gave way to a callback that traps",
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_the_set_a_queued_callback_takes_its_event_from() {
    assert_fails_with(
        &cancel_failing_in(SET_DROPPED, "drop-under-the-queued-callback").await,
        "cannot drop waitable set with waiters",
        "a drop of the set a queued callback waits to take its event from",
    );
}

#[wcmp_macros::test]
async fn it_lets_a_callback_drop_the_set_it_took_its_event_from() {
    assert_eq!(
        cancel_in(SET_DROPPED, "wait-then-drop").await,
        7,
        "the callback took its event, dropped the set, and returned"
    );
}

#[wcmp_macros::test]
async fn it_finishes_a_cancel_whose_woken_callee_switches_to_a_suspended_thread() {
    assert_eq!(
        cancel_in(SWITCHES, "cancel-a-callee-that-switches").await,
        CANCELLED_BEFORE_RETURNED,
        "the woken callback switches to its helper, and confirms once the helper made it ready"
    );
}

#[wcmp_macros::test]
async fn it_does_not_lend_the_handle_to_a_call_the_gate_holds() {
    assert_eq!(
        cancel_in(LENDS, "drop-while-gated").await,
        CANCELLED_BEFORE_STARTED,
        "the gated call has not lifted its borrow, so the handle drops before the cancel"
    );
}

/// Whether each host-callee test runs with the suspend provider on,
/// and then with it off. With it on, a blocked thread suspends through
/// the provider the target has; with it off, a block runs a nested
/// turn above the blocked call. Either way the future is dropped in a
/// turn's poll of the host tasks.
const PROVIDERS: [bool; 2] = [true, false];

/// What the host side of [`HOST_CALLS`] saw of the futures of `never`
/// it dropped.
#[derive(Default)]
struct Drops {
    /// How many were dropped.
    dropped: AtomicU32,
    /// How many of those drops reached the store through the accessor
    /// the future kept.
    reached: AtomicU32,
}

/// The future of one call of `never`: pending for ever, holding the
/// accessor its call was handed.
///
/// Its drop reaches the store through that accessor and counts the
/// reach in the host data. The reach succeeds only where the store is
/// lent to a poll of the host tasks, which is inside a turn. A drop
/// inside the built-in finds no store lent, and the reach fails.
struct Never {
    accessor: Accessor<u32>,
    drops: Arc<Drops>,
}

impl Future for Never {
    type Output = Result<u32, Error>;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for Never {
    fn drop(&mut self) {
        if self.accessor.with(|store| *store.data_mut() += 1).is_ok() {
            self.drops.reached.fetch_add(1, Ordering::SeqCst);
        }
        self.drops.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// The future of one call of `echo`: pending once, having asked to be
/// polled again, and then its argument.
struct EchoOnce {
    value: u32,
    polled: bool,
}

impl Future for EchoOnce {
    type Output = Result<u32, Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.polled {
            return Poll::Ready(Ok(self.value));
        }
        self.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Instantiate [`HOST_CALLS`] in a store of its own, whose host data
/// counts the drops that reached it, with the suspend provider on or
/// off as `provider` says. The host functions report to `drops`.
async fn instantiate_host_calls(provider: bool, drops: &Arc<Drops>) -> (Store<u32>, Instance) {
    let mut config = EngineConfig::new();
    config
        .suspend_provider(provider)
        .wasm_component_model_more_async_builtins(true);
    let engine = Engine::with_config(&config).expect("engine");
    let component = Component::new(&engine, HOST_CALLS)
        .await
        .expect("the component translates");
    let mut linker: Linker<u32> = Linker::new(&engine);
    let mut root = linker.root();
    root.func_wrap_concurrent("never", {
        let drops = drops.clone();
        move |accessor: &Accessor<u32>, (): ()| Never {
            accessor: accessor.clone(),
            drops: drops.clone(),
        }
    })
    .expect("the registration of `never`");
    root.func_wrap_concurrent("echo", |_: &Accessor<u32>, (value,): (u32,)| EchoOnce {
        value,
        polled: false,
    })
    .expect("the registration of `echo`");
    root.func_wrap("dropped", {
        let drops = drops.clone();
        move |_, (): ()| Ok(drops.dropped.load(Ordering::SeqCst))
    })
    .expect("the registration of `dropped`");
    let mut store: Store<u32> = Store::new(&engine, 0).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

/// Call the export `name` of a fresh instance of [`HOST_CALLS`] and
/// answer the `u32` fields of the tuple it returned, with the store.
async fn host_cancel(provider: bool, drops: &Arc<Drops>, name: &str) -> (Vec<u32>, Store<u32>) {
    let (mut store, instance) = instantiate_host_calls(provider, drops).await;
    let func = instance.get_func(name).expect("the export is declared");
    let values = func
        .call(&mut store, &[])
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {}", chain(&error)))
        .into_vec();
    let [Val::Tuple(fields)] = values.as_slice() else {
        panic!("{name} answered {values:?} rather than one tuple");
    };
    let fields = fields
        .iter()
        .map(|field| match field {
            Val::U32(value) => *value,
            other => panic!("{name} answered a field {other:?} rather than a u32"),
        })
        .collect();
    (fields, store)
}

#[wcmp_macros::test]
async fn it_answers_blocked_to_an_asynchronous_cancel_of_a_host_callee_then_drops_in_a_turn() {
    for provider in PROVIDERS {
        let drops = Arc::new(Drops::default());
        let (fields, store) = host_cancel(provider, &drops, "cancel-async").await;
        assert_eq!(
            fields,
            [BLOCKED, 0, CANCELLED_BEFORE_RETURNED],
            "provider {provider}: the cancel answers BLOCKED and drops nothing, and the \
             subtask event then carries CANCELLED_BEFORE_RETURNED"
        );
        assert_eq!(
            drops.dropped.load(Ordering::SeqCst),
            1,
            "provider {provider}"
        );
        assert_eq!(
            drops.reached.load(Ordering::SeqCst),
            1,
            "provider {provider}: the drop reached the store, so it ran in a turn's poll of \
             the host tasks and not inside the built-in"
        );
        assert_eq!(
            *store.data(),
            1,
            "provider {provider}: the reach ran against this store"
        );
    }
}

#[wcmp_macros::test]
async fn it_blocks_a_synchronous_cancel_of_a_host_callee_until_a_turn_drops_its_future() {
    for provider in PROVIDERS {
        let drops = Arc::new(Drops::default());
        let (fields, store) = host_cancel(provider, &drops, "cancel-sync").await;
        assert_eq!(
            fields,
            [CANCELLED_BEFORE_RETURNED, 1],
            "provider {provider}: the cancel returns once the future is dropped"
        );
        assert_eq!(
            drops.reached.load(Ordering::SeqCst),
            1,
            "provider {provider}: the drop reached the store, so it ran in a turn's poll of \
             the host tasks and not inside the built-in"
        );
        assert_eq!(
            *store.data(),
            1,
            "provider {provider}: the reach ran against this store"
        );
    }
}

#[wcmp_macros::test]
async fn it_resolves_a_host_callee_whose_future_completed_before_the_cancel_as_returned() {
    for provider in PROVIDERS {
        let drops = Arc::new(Drops::default());
        let (fields, _store) = host_cancel(provider, &drops, "cancel-returned").await;
        assert_eq!(
            fields,
            [RETURNED, RETURNED, 7],
            "provider {provider}: the first call returned with the second, so the cancel \
             answers RETURNED at once and the call's result lowered as usual"
        );
    }
}
