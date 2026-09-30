//! Baseline tests for the `cancellable` immediate of
//! `waitable-set.wait`, `waitable-set.poll`, and the thread built-ins.
//!
//! The reference removed the immediate, but Wasmtime 49 still reads it
//! and honors it, and the C generator of wit-bindgen still emits it.
//! The polyfill honors it as Wasmtime does, so that one guest binary
//! behaves the same in both:
//!
//! - Every cancellable built-in first takes a pending cancellation
//!   request and answers the cancelled result at once: the
//!   task-cancelled event (6) from a wait or a poll, and 1 from a
//!   thread built-in. A wait or a poll does so even when its set holds
//!   an event, and the set keeps the event.
//! - A cancellable wait that blocks is woken by `subtask.cancel`, which
//!   runs it ahead of work that was ready before it, and it answers
//!   the task-cancelled event. A wait whose set already holds an event
//!   is not woken: the event made it ready, and it runs in its turn,
//!   where it still answers the task-cancelled event first.
//! - A thread in a cancellable yield, from `thread.yield` or from a
//!   promote that yields, is run first by `subtask.cancel`, ahead of a
//!   yield that became ready before it, and answers 1.
//! - A thread in a cancellable suspension, from `thread.suspend` or
//!   from a switch that suspends, is not woken by `subtask.cancel`.
//!   Once another thread resumes it, it answers 1 for the request
//!   still pending.
//! - A built-in without the immediate never takes a request, and
//!   answers what it answers without one.
//!
//! The text format no longer spells the immediate: `wast` refuses the
//! `cancellable` keyword, and encodes the byte that carried it as
//! zero. The fixture is therefore assembled with that byte zero, and
//! [`cancellable`] sets it for the first built-in of each kind the
//! binary defines, which is how a toolchain that still emits the
//! immediate encodes it.
//!
//! Every rule is proved with the engine's suspend provider, which
//! suspends a thread on a stack of its own on both targets, and each
//! test asserts the provider is there. Three more tests turn the
//! provider off, where a cancellable built-in blocks or gives way in a
//! nested turn on the real stack. A request made by work inside that
//! turn ends the wait with the task-cancelled event, and a yield then
//! answers 1. A request from the caller's frame below cannot come
//! until the callee gives control back, and the wait fails with the
//! stack-switch cause.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::task::Poll;

use wasmparser::{CanonicalFunction, Parser, Payload};
use wcmp::{
    Component, Engine, EngineConfig, Error, Instance, Linker, SchedulerCause, Store,
    SuspendProviderKind, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The subtask state of a callee that returned its result.
const RETURNED: u32 = 2;
/// The subtask state of a callee that confirmed its cancellation.
const CANCELLED_BEFORE_RETURNED: u32 = 4;
/// What `subtask.cancel` answers when the callee has not resolved.
const BLOCKED: u32 = 0xffff_ffff;

/// Which cancellable built-in the callee's `pending` export calls
/// once a request is pending, in the order its dispatch reads them.
const PENDING_BUILT_INS: [(u32, &str); 8] = [
    (0, "waitable-set.wait"),
    (1, "waitable-set.poll"),
    (2, "thread.yield"),
    (3, "thread.suspend"),
    (4, "thread.suspend-then-resume"),
    (5, "thread.yield-then-resume"),
    (6, "thread.suspend-then-promote"),
    (7, "thread.yield-then-promote"),
];

/// A callee `$C` whose stackful exports call the cancellable
/// built-ins, and a caller `$D` that cancels calls into it.
///
/// `$C` defines each of the eight built-ins that can carry the
/// immediate first as `$c-...`, which [`cancellable`] makes
/// cancellable, and then, for five of them, again without it.
///
/// - `pending` gives its caller a turn with a yield that is not
///   cancellable, in which the caller asks it to stop, and then calls
///   the cancellable built-in its argument selects. Each must answer
///   the cancelled result at once: a wait on an empty set would block,
///   a poll would answer none, a yield or a suspension would give way
///   and answer zero, and a switch would trap on the thread index zero,
///   which names no thread. It then confirms. With argument 8 or 9 it
///   calls the cancellable wait or poll on a set that holds a write's
///   event instead, and traps unless a plain poll then finds the event.
/// - `wait-woken` waits on an empty set with the cancellable wait, and
///   confirms once the wait answers the task-cancelled event.
/// - `event-waiter` waits, without the immediate, on a set whose event
///   `drop-reader` makes ready, counts that it went on, and returns
///   `7`. `wait-first` waits on an empty set with the cancellable wait,
///   traps unless it answered the task-cancelled event before any
///   `event-waiter` went on, and confirms. `wait-ready` waits with the
///   cancellable wait on a set of its own whose event `drop-reader`
///   makes ready, traps unless the wait answered the task-cancelled
///   event after one `event-waiter` went on and the set still holds its
///   event, and confirms.
/// - `wait-later` is lifted with a callback. It waits in its event loop
///   for a write's event, and its callback then calls the cancellable
///   wait on an empty set, the cancellable `thread.yield`, or the
///   cancellable `thread.yield-then-promote` of a thread that is not
///   ready, as its argument selects. It traps unless that answered the
///   cancelled result, and confirms.
/// - `yield-plain` yields without the immediate, counts that it went
///   on, and returns `7`. `yield-first` yields with the cancellable
///   `thread.yield`, or with the cancellable `thread.yield-then-promote`
///   of a thread that is not ready, traps unless it answered 1 and went
///   on before any `yield-plain`, and confirms.
/// - `suspend-held` suspends with the cancellable `thread.suspend`, or
///   with the cancellable `thread.suspend-then-promote` of a thread
///   that is not ready, traps unless it answered 1, and confirms.
///   `resume-main` resumes it.
/// - `never-takes` gives its caller the same turn, then calls a poll, a
///   yield, a promote that yields, a wait on a set that holds an event,
///   and a suspension that a thread of its own resumes, none of them
///   cancellable, traps unless each answered what it answers with no
///   request pending, and returns `7`.
///
/// Each export of `$D` makes the calls its name says through an
/// asynchronous lower, cancels, and answers the state the cancelled
/// subtask resolved to: what the cancel answered, or, when that was
/// `BLOCKED`, the state the subtask event later delivers. `wait-first`
/// answers what the cancel answered. `cancel-later`, `trigger`, and
/// `finish` split one cancel of `wait-later` across three calls, as
/// [`cancel_later`] states.
const CANCELLABLE: &[u8] = component!(
    r#"
    (component
      (component $C
        (core module $Libc
          (memory (export "mem") 1)
          (table (export "table") 2 funcref))
        (core instance $libc (instantiate $Libc))
        (core type $start-ty (func (param i32)))
        (alias core export $libc "table" (core table $table))
        (type $f (future u32))
        (core func $c-wait (canon waitable-set.wait (memory (core memory $libc "mem"))))
        (core func $c-poll (canon waitable-set.poll (memory (core memory $libc "mem"))))
        (core func $c-yield (canon thread.yield))
        (core func $c-suspend (canon thread.suspend))
        (core func $c-suspend-then-resume (canon thread.suspend-then-resume))
        (core func $c-yield-then-resume (canon thread.yield-then-resume))
        (core func $c-suspend-then-promote (canon thread.suspend-then-promote))
        (core func $c-yield-then-promote (canon thread.yield-then-promote))
        (core func $wait (canon waitable-set.wait (memory (core memory $libc "mem"))))
        (core func $poll (canon waitable-set.poll (memory (core memory $libc "mem"))))
        (core func $yield (canon thread.yield))
        (core func $suspend (canon thread.suspend))
        (core func $yield-then-promote (canon thread.yield-then-promote))
        (core func $task.cancel (canon task.cancel))
        (core func $task.return (canon task.return (result u32)))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.drop (canon waitable-set.drop))
        (core func $waitable.join (canon waitable.join))
        (core func $thread.index (canon thread.index))
        (core func $thread.new-indirect (canon thread.new-indirect $start-ty (core table $table)))
        (core func $thread.resume-later (canon thread.resume-later))
        (core func $future.new (canon future.new $f))
        (core func $future.write (canon future.write $f async (memory (core memory $libc "mem"))))
        (core func $future.drop-readable (canon future.drop-readable $f))
        (core func $future.drop-writable (canon future.drop-writable $f))
        (core module $CM
          (import "" "mem" (memory 1))
          (import "" "table" (table 2 funcref))
          (import "" "c-wait" (func $c-wait (param i32 i32) (result i32)))
          (import "" "c-poll" (func $c-poll (param i32 i32) (result i32)))
          (import "" "c-yield" (func $c-yield (result i32)))
          (import "" "c-suspend" (func $c-suspend (result i32)))
          (import "" "c-suspend-then-resume" (func $c-suspend-then-resume (param i32) (result i32)))
          (import "" "c-yield-then-resume" (func $c-yield-then-resume (param i32) (result i32)))
          (import "" "c-suspend-then-promote"
            (func $c-suspend-then-promote (param i32) (result i32)))
          (import "" "c-yield-then-promote" (func $c-yield-then-promote (param i32) (result i32)))
          (import "" "wait" (func $wait (param i32 i32) (result i32)))
          (import "" "poll" (func $poll (param i32 i32) (result i32)))
          (import "" "yield" (func $yield (result i32)))
          (import "" "suspend" (func $suspend (result i32)))
          (import "" "yield-then-promote" (func $yield-then-promote (param i32) (result i32)))
          (import "" "task.cancel" (func $task.cancel))
          (import "" "task.return" (func $task.return (param i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.drop" (func $waitable-set.drop (param i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "thread.index" (func $thread.index (result i32)))
          (import "" "thread.new-indirect" (func $thread.new-indirect (param i32 i32) (result i32)))
          (import "" "thread.resume-later" (func $thread.resume-later (param i32)))
          (import "" "future.new" (func $future.new (result i64)))
          (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
          (import "" "future.drop-readable" (func $future.drop-readable (param i32)))
          (import "" "future.drop-writable" (func $future.drop-writable (param i32)))

          ;; A set that never holds an event.
          (global $ws (mut i32) (i32.const 0))
          ;; The thread `resume-main` and the resumer thread resume.
          (global $main (mut i32) (i32.const 0))
          ;; How many yielders or waiters have gone on.
          (global $order (mut i32) (i32.const 0))
          ;; Which built-in `wait-later-cb` calls.
          (global $later (mut i32) (i32.const 0))
          (func $start (global.set $ws (call $waitable-set.new)))
          (start $start)

          ;; The start function of a thread that does nothing.
          (func $noop (param i32))
          ;; The start function of a thread that resumes `$main`.
          (func $resumer (param i32) (call $thread.resume-later (global.get $main)))
          (elem (table 0) (i32.const 0) func $noop $resumer)

          (func $expect (param $got i32) (param $want i32)
            (if (i32.ne (local.get $got) (local.get $want))
              (then unreachable)))
          ;; A thread of the current task that is suspended and never
          ;; ready until `$finish-idle` lets it run and end.
          (func $idle-thread (result i32)
            (call $thread.new-indirect (i32.const 0) (i32.const 0)))
          (func $finish-idle (param $thread i32)
            (call $thread.resume-later (local.get $thread)))
          ;; A wait or a poll that took a request delivers two zero
          ;; payloads with the task-cancelled event.
          (func $expect-cancelled-event (param $code i32)
            (call $expect (local.get $code) (i32.const 6 (; TASK_CANCELLED ;)))
            (call $expect (i32.load (i32.const 16)) (i32.const 0))
            (call $expect (i32.load (i32.const 20)) (i32.const 0)))
          ;; A new set that holds the writable end of a future whose
          ;; write blocks, until `drop-reader` drops the readable end,
          ;; kept in slot `which`, and leaves the write's event.
          (func $set-with-write (param $which i32) (result i32)
            (local $ends i64) (local $writable i32) (local $set i32)
            (local.set $ends (call $future.new))
            (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
            (call $expect (call $future.write (local.get $writable) (i32.const 32))
                          (i32.const -1 (; BLOCKED ;)))
            (i32.store (i32.add (i32.const 48) (i32.shl (local.get $which) (i32.const 2)))
                       (i32.wrap_i64 (local.get $ends)))
            (local.set $set (call $waitable-set.new))
            (call $waitable.join (local.get $writable) (local.get $set))
            (local.get $set))
          (func $drop-reader (export "drop-reader") (param $which i32)
            (call $future.drop-readable
              (i32.load (i32.add (i32.const 48) (i32.shl (local.get $which) (i32.const 2))))))

          (func (export "pending") (param $which i32)
            (local $set i32)
            (i32.store (i32.const 16) (i32.const -1))
            (i32.store (i32.const 20) (i32.const -1))
            (call $expect (call $yield) (i32.const 0))
            (if (i32.eq (local.get $which) (i32.const 0))
              (then (call $expect-cancelled-event (call $c-wait (global.get $ws) (i32.const 16)))))
            (if (i32.eq (local.get $which) (i32.const 1))
              (then (call $expect-cancelled-event (call $c-poll (global.get $ws) (i32.const 16)))))
            (if (i32.eq (local.get $which) (i32.const 2))
              (then (call $expect (call $c-yield) (i32.const 1))))
            (if (i32.eq (local.get $which) (i32.const 3))
              (then (call $expect (call $c-suspend) (i32.const 1))))
            (if (i32.eq (local.get $which) (i32.const 4))
              (then (call $expect (call $c-suspend-then-resume (i32.const 0)) (i32.const 1))))
            (if (i32.eq (local.get $which) (i32.const 5))
              (then (call $expect (call $c-yield-then-resume (i32.const 0)) (i32.const 1))))
            (if (i32.eq (local.get $which) (i32.const 6))
              (then (call $expect (call $c-suspend-then-promote (i32.const 0)) (i32.const 1))))
            (if (i32.eq (local.get $which) (i32.const 7))
              (then (call $expect (call $c-yield-then-promote (i32.const 0)) (i32.const 1))))
            (if (i32.ge_u (local.get $which) (i32.const 8))
              (then
                (local.set $set (call $set-with-write (i32.const 1)))
                (call $drop-reader (i32.const 1))
                (if (i32.eq (local.get $which) (i32.const 8))
                  (then (call $expect-cancelled-event (call $c-wait (local.get $set) (i32.const 16))))
                  (else (call $expect-cancelled-event (call $c-poll (local.get $set) (i32.const 16)))))
                ;; The set kept its event.
                (call $expect (call $poll (local.get $set) (i32.const 16))
                              (i32.const 5 (; FUTURE_WRITE ;)))))
            (call $task.cancel))

          (func (export "event-waiter")
            (call $expect (call $wait (call $set-with-write (i32.const 0)) (i32.const 24))
                          (i32.const 5 (; FUTURE_WRITE ;)))
            (global.set $order (i32.add (global.get $order) (i32.const 1)))
            (call $task.return (i32.const 7)))

          (func (export "wait-first")
            (i32.store (i32.const 16) (i32.const -1))
            (i32.store (i32.const 20) (i32.const -1))
            (call $expect-cancelled-event (call $c-wait (global.get $ws) (i32.const 16)))
            (call $expect (global.get $order) (i32.const 0))
            (global.set $order (i32.add (global.get $order) (i32.const 1)))
            (call $task.cancel))

          (func (export "wait-ready")
            (local $set i32)
            (local.set $set (call $set-with-write (i32.const 1)))
            (i32.store (i32.const 16) (i32.const -1))
            (i32.store (i32.const 20) (i32.const -1))
            (call $expect-cancelled-event (call $c-wait (local.get $set) (i32.const 16)))
            (call $expect (global.get $order) (i32.const 1))
            (call $expect (call $poll (local.get $set) (i32.const 16))
                          (i32.const 5 (; FUTURE_WRITE ;)))
            (call $task.cancel))

          (func (export "wait-later") (param $which i32) (result i32)
            (global.set $later (local.get $which))
            (i32.or (i32.const 2 (; WAIT ;))
                    (i32.shl (call $set-with-write (i32.const 0)) (i32.const 4))))
          (func (export "wait-later-cb") (param $event i32) (param i32 i32) (result i32)
            (local $idle i32)
            (call $expect (local.get $event) (i32.const 5 (; FUTURE_WRITE ;)))
            (i32.store (i32.const 16) (i32.const -1))
            (i32.store (i32.const 20) (i32.const -1))
            (if (i32.eqz (global.get $later))
              (then (call $expect-cancelled-event (call $c-wait (global.get $ws) (i32.const 16)))))
            (if (i32.eq (global.get $later) (i32.const 1))
              (then (call $expect (call $c-yield) (i32.const 1))))
            (if (i32.eq (global.get $later) (i32.const 2))
              (then
                (local.set $idle (call $idle-thread))
                (call $expect (call $c-yield-then-promote (local.get $idle)) (i32.const 1))
                (call $finish-idle (local.get $idle))))
            (call $task.cancel)
            (i32.const 0 (; EXIT ;)))

          (func (export "wait-woken")
            (i32.store (i32.const 16) (i32.const -1))
            (i32.store (i32.const 20) (i32.const -1))
            (call $expect-cancelled-event (call $c-wait (global.get $ws) (i32.const 16)))
            (call $task.cancel))

          (func (export "yield-plain")
            (call $expect (call $yield) (i32.const 0))
            (global.set $order (i32.add (global.get $order) (i32.const 1)))
            (call $task.return (i32.const 7)))

          (func (export "yield-first") (param $which i32)
            (local $idle i32)
            (if (i32.eqz (local.get $which))
              (then (call $expect (call $c-yield) (i32.const 1)))
              (else
                (local.set $idle (call $idle-thread))
                (call $expect (call $c-yield-then-promote (local.get $idle)) (i32.const 1))
                (call $finish-idle (local.get $idle))))
            (call $expect (global.get $order) (i32.const 0))
            (global.set $order (i32.add (global.get $order) (i32.const 1)))
            (call $task.cancel))

          (func (export "suspend-held") (param $which i32)
            (local $idle i32)
            (global.set $main (call $thread.index))
            (if (i32.eqz (local.get $which))
              (then (call $expect (call $c-suspend) (i32.const 1)))
              (else
                (local.set $idle (call $idle-thread))
                (call $expect (call $c-suspend-then-promote (local.get $idle)) (i32.const 1))
                (call $finish-idle (local.get $idle))))
            (call $task.cancel))

          (func (export "resume-main")
            (call $thread.resume-later (global.get $main)))

          (func (export "never-takes")
            (local $idle i32) (local $ends i64) (local $writable i32) (local $set i32)
            (call $expect (call $yield) (i32.const 0))
            ;; A request is pending from here on.
            (call $expect (call $poll (global.get $ws) (i32.const 16)) (i32.const 0 (; NONE ;)))
            (call $expect (call $yield) (i32.const 0))
            (local.set $idle (call $idle-thread))
            (call $expect (call $yield-then-promote (local.get $idle)) (i32.const 0))
            (call $finish-idle (local.get $idle))
            ;; A write whose reader is dropped leaves its event on the
            ;; writable end, in a set of its own.
            (local.set $ends (call $future.new))
            (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
            (call $expect (call $future.write (local.get $writable) (i32.const 32))
                          (i32.const -1 (; BLOCKED ;)))
            (call $future.drop-readable (i32.wrap_i64 (local.get $ends)))
            (local.set $set (call $waitable-set.new))
            (call $waitable.join (local.get $writable) (local.get $set))
            (call $expect (call $wait (local.get $set) (i32.const 16))
                          (i32.const 5 (; FUTURE_WRITE ;)))
            (call $expect (i32.load (i32.const 16)) (local.get $writable))
            (call $waitable.join (local.get $writable) (i32.const 0))
            (call $waitable-set.drop (local.get $set))
            (call $future.drop-writable (local.get $writable))
            ;; A thread of this task resumes the suspension.
            (global.set $main (call $thread.index))
            (call $thread.resume-later (call $thread.new-indirect (i32.const 1) (i32.const 0)))
            (call $expect (call $suspend) (i32.const 0))
            (call $task.return (i32.const 7))))
        (core instance $cm (instantiate $CM (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "table" (table $table))
          (export "c-wait" (func $c-wait))
          (export "c-poll" (func $c-poll))
          (export "c-yield" (func $c-yield))
          (export "c-suspend" (func $c-suspend))
          (export "c-suspend-then-resume" (func $c-suspend-then-resume))
          (export "c-yield-then-resume" (func $c-yield-then-resume))
          (export "c-suspend-then-promote" (func $c-suspend-then-promote))
          (export "c-yield-then-promote" (func $c-yield-then-promote))
          (export "wait" (func $wait))
          (export "poll" (func $poll))
          (export "yield" (func $yield))
          (export "suspend" (func $suspend))
          (export "yield-then-promote" (func $yield-then-promote))
          (export "task.cancel" (func $task.cancel))
          (export "task.return" (func $task.return))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.drop" (func $waitable-set.drop))
          (export "waitable.join" (func $waitable.join))
          (export "thread.index" (func $thread.index))
          (export "thread.new-indirect" (func $thread.new-indirect))
          (export "thread.resume-later" (func $thread.resume-later))
          (export "future.new" (func $future.new))
          (export "future.write" (func $future.write))
          (export "future.drop-readable" (func $future.drop-readable))
          (export "future.drop-writable" (func $future.drop-writable))))))
        (func (export "pending") async (param "w" u32) (result u32)
          (canon lift (core func $cm "pending") async))
        (func (export "wait-woken") async (result u32)
          (canon lift (core func $cm "wait-woken") async))
        (func (export "event-waiter") async (result u32)
          (canon lift (core func $cm "event-waiter") async))
        (func (export "wait-first") async (result u32)
          (canon lift (core func $cm "wait-first") async))
        (func (export "wait-ready") async (result u32)
          (canon lift (core func $cm "wait-ready") async))
        (func (export "wait-later") async (param "w" u32) (result u32)
          (canon lift (core func $cm "wait-later") async (callback (core func $cm "wait-later-cb"))))
        (func (export "drop-reader") (param "w" u32)
          (canon lift (core func $cm "drop-reader")))
        (func (export "yield-plain") async (result u32)
          (canon lift (core func $cm "yield-plain") async))
        (func (export "yield-first") async (param "w" u32) (result u32)
          (canon lift (core func $cm "yield-first") async))
        (func (export "suspend-held") async (param "w" u32) (result u32)
          (canon lift (core func $cm "suspend-held") async))
        (func (export "resume-main") (canon lift (core func $cm "resume-main")))
        (func (export "never-takes") async (result u32)
          (canon lift (core func $cm "never-takes") async)))

      (component $D
        (import "pending" (func $pending async (param "w" u32) (result u32)))
        (import "wait-woken" (func $wait-woken async (result u32)))
        (import "event-waiter" (func $event-waiter async (result u32)))
        (import "wait-first" (func $wait-first async (result u32)))
        (import "wait-ready" (func $wait-ready async (result u32)))
        (import "wait-later" (func $wait-later async (param "w" u32) (result u32)))
        (import "drop-reader" (func $drop-reader (param "w" u32)))
        (import "yield-plain" (func $yield-plain async (result u32)))
        (import "yield-first" (func $yield-first async (param "w" u32) (result u32)))
        (import "suspend-held" (func $suspend-held async (param "w" u32) (result u32)))
        (import "resume-main" (func $resume-main))
        (import "never-takes" (func $never-takes async (result u32)))
        (core module $Memory (memory (export "mem") 1))
        (core instance $memory (instantiate $Memory))
        (type $f (future u32))
        (core func $future.new (canon future.new $f))
        (core func $future.write
          (canon future.write $f async (memory (core memory $memory "mem"))))
        (core func $future.drop-readable (canon future.drop-readable $f))
        (core func $task.return (canon task.return (result u32)))
        (core func $cancel-async (canon subtask.cancel async))
        (core func $cancel-sync (canon subtask.cancel))
        (core func $subtask.drop (canon subtask.drop))
        (core func $waitable.join (canon waitable.join))
        (core func $waitable-set.new (canon waitable-set.new))
        (core func $waitable-set.drop (canon waitable-set.drop))
        (core func $waitable-set.wait
          (canon waitable-set.wait (memory (core memory $memory "mem"))))
        (core func $pending'
          (canon lower (func $pending) async (memory (core memory $memory "mem"))))
        (core func $wait-woken'
          (canon lower (func $wait-woken) async (memory (core memory $memory "mem"))))
        (core func $event-waiter'
          (canon lower (func $event-waiter) async (memory (core memory $memory "mem"))))
        (core func $wait-first'
          (canon lower (func $wait-first) async (memory (core memory $memory "mem"))))
        (core func $wait-ready'
          (canon lower (func $wait-ready) async (memory (core memory $memory "mem"))))
        (core func $wait-later'
          (canon lower (func $wait-later) async (memory (core memory $memory "mem"))))
        (core func $drop-reader' (canon lower (func $drop-reader)))
        (core func $yield-plain'
          (canon lower (func $yield-plain) async (memory (core memory $memory "mem"))))
        (core func $yield-first'
          (canon lower (func $yield-first) async (memory (core memory $memory "mem"))))
        (core func $suspend-held'
          (canon lower (func $suspend-held) async (memory (core memory $memory "mem"))))
        (core func $resume-main' (canon lower (func $resume-main)))
        (core func $never-takes'
          (canon lower (func $never-takes) async (memory (core memory $memory "mem"))))
        (core module $DM
          (import "" "mem" (memory 1))
          (import "" "cancel-async" (func $cancel-async (param i32) (result i32)))
          (import "" "cancel-sync" (func $cancel-sync (param i32) (result i32)))
          (import "" "subtask.drop" (func $subtask.drop (param i32)))
          (import "" "waitable.join" (func $waitable.join (param i32 i32)))
          (import "" "waitable-set.new" (func $waitable-set.new (result i32)))
          (import "" "waitable-set.drop" (func $waitable-set.drop (param i32)))
          (import "" "waitable-set.wait" (func $waitable-set.wait (param i32 i32) (result i32)))
          (import "" "pending" (func $pending (param i32 i32) (result i32)))
          (import "" "wait-woken" (func $wait-woken (param i32) (result i32)))
          (import "" "event-waiter" (func $event-waiter (param i32) (result i32)))
          (import "" "wait-first" (func $wait-first (param i32) (result i32)))
          (import "" "wait-ready" (func $wait-ready (param i32) (result i32)))
          (import "" "wait-later" (func $wait-later (param i32 i32) (result i32)))
          (import "" "drop-reader" (func $drop-reader (param i32)))
          (import "" "future.new" (func $future.new (result i64)))
          (import "" "future.write" (func $future.write (param i32 i32) (result i32)))
          (import "" "future.drop-readable" (func $future.drop-readable (param i32)))
          (import "" "task.return" (func $task.return (param i32)))
          (import "" "yield-plain" (func $yield-plain (param i32) (result i32)))
          (import "" "yield-first" (func $yield-first (param i32 i32) (result i32)))
          (import "" "suspend-held" (func $suspend-held (param i32 i32) (result i32)))
          (import "" "resume-main" (func $resume-main))
          (import "" "never-takes" (func $never-takes (param i32) (result i32)))

          ;; The subtask `cancel-later` started and cancels.
          (global $later (mut i32) (i32.const 0))
          ;; The readable end `trigger` drops to run `cancel-later-cb`.
          (global $reader (mut i32) (i32.const 0))

          ;; The subtask index a lower's status word carries, once the
          ;; call has started.
          (func $started (param $status i32) (result i32)
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1 (; STARTED ;)))
              (then unreachable))
            (i32.shr_u (local.get $status) (i32.const 4)))
          ;; The state `sub` resolves to, from the subtask event a wait
          ;; delivers.
          (func $wait-for (param $sub i32) (result i32)
            (local $ws i32) (local $state i32)
            (local.set $ws (call $waitable-set.new))
            (call $waitable.join (local.get $sub) (local.get $ws))
            (if (i32.ne (call $waitable-set.wait (local.get $ws) (i32.const 8))
                        (i32.const 1 (; SUBTASK ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 8)) (local.get $sub))
              (then unreachable))
            (local.set $state (i32.load (i32.const 12)))
            (call $waitable.join (local.get $sub) (i32.const 0))
            (call $waitable-set.drop (local.get $ws))
            (local.get $state))
          ;; The state a cancelled `sub` resolves to, given what the
          ;; cancel answered. The subtask is dropped once it resolved.
          (func $resolution (param $sub i32) (param $status i32) (result i32)
            (if (i32.eq (local.get $status) (i32.const -1 (; BLOCKED ;)))
              (then (local.set $status (call $wait-for (local.get $sub)))))
            (call $subtask.drop (local.get $sub))
            (local.get $status))

          (func (export "pending") (param $which i32) (result i32)
            (local $sub i32)
            (local.set $sub (call $started (call $pending (local.get $which) (i32.const 0))))
            (call $resolution (local.get $sub) (call $cancel-async (local.get $sub))))

          (func (export "wait-woken-async") (result i32)
            (local $sub i32) (local $status i32)
            (local.set $sub (call $started (call $wait-woken (i32.const 0))))
            (local.set $status (call $cancel-async (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (local.get $status))

          (func (export "wait-woken-sync") (result i32)
            (local $sub i32) (local $status i32)
            (local.set $sub (call $started (call $wait-woken (i32.const 0))))
            (local.set $status (call $cancel-sync (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (local.get $status))

          ;; `event-waiter` is made ready by an event before the cancel,
          ;; so a cancel that did not run `wait-first` first would run
          ;; `event-waiter` ahead of it, and `wait-first` would trap.
          (func (export "wait-first") (param $sync i32) (result i32)
            (local $ready i32) (local $sub i32) (local $status i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $ready (call $started (call $event-waiter (i32.const 0))))
            (local.set $sub (call $started (call $wait-first (i32.const 4))))
            (call $drop-reader (i32.const 0))
            (local.set $status
              (if (result i32) (local.get $sync)
                (then (call $cancel-sync (local.get $sub)))
                (else (call $cancel-async (local.get $sub)))))
            (if (i32.ne (local.get $status) (i32.const -1 (; BLOCKED ;)))
              (then (call $subtask.drop (local.get $sub))))
            (if (i32.ne (call $wait-for (local.get $ready)) (i32.const 2 (; RETURNED ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 0)) (i32.const 7))
              (then unreachable))
            (call $subtask.drop (local.get $ready))
            (local.get $status))

          ;; Both events come before the cancel: `event-waiter` is made
          ;; ready first, then `wait-ready`.
          (func (export "wait-ready") (result i32)
            (local $ready i32) (local $sub i32) (local $state i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $ready (call $started (call $event-waiter (i32.const 0))))
            (local.set $sub (call $started (call $wait-ready (i32.const 4))))
            (call $drop-reader (i32.const 0))
            (call $drop-reader (i32.const 1))
            (local.set $state
              (call $resolution (local.get $sub) (call $cancel-async (local.get $sub))))
            (if (i32.ne (call $wait-for (local.get $ready)) (i32.const 2 (; RETURNED ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 0)) (i32.const 7))
              (then unreachable))
            (call $subtask.drop (local.get $ready))
            (local.get $state))

          ;; Start `wait-later`, which waits in its event loop, and wait
          ;; in this task's own loop on a write that `trigger` makes
          ;; ready.
          (func (export "cancel-later") (param $which i32) (result i32)
            (local $ends i64) (local $writable i32) (local $set i32)
            (global.set $later (call $started (call $wait-later (local.get $which) (i32.const 0))))
            (local.set $ends (call $future.new))
            (local.set $writable (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32))))
            (if (i32.ne (call $future.write (local.get $writable) (i32.const 64))
                        (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (global.set $reader (i32.wrap_i64 (local.get $ends)))
            (local.set $set (call $waitable-set.new))
            (call $waitable.join (local.get $writable) (local.get $set))
            (i32.or (i32.const 2 (; WAIT ;)) (i32.shl (local.get $set) (i32.const 4))))
          ;; Cancel `wait-later` from inside the turn its built-in
          ;; blocks in, and return what the cancel answered.
          (func (export "cancel-later-cb") (param $event i32) (param i32 i32) (result i32)
            (if (i32.ne (local.get $event) (i32.const 5 (; FUTURE_WRITE ;)))
              (then unreachable))
            (call $task.return (call $cancel-async (global.get $later)))
            (i32.const 0 (; EXIT ;)))
          ;; Make `wait-later`'s event loop ready, then this instance's.
          (func (export "trigger")
            (call $drop-reader (i32.const 0))
            (call $future.drop-readable (global.get $reader)))
          ;; The state `wait-later` resolved to.
          (func (export "finish") (result i32)
            (local $state i32)
            (local.set $state (call $wait-for (global.get $later)))
            (call $subtask.drop (global.get $later))
            (local.get $state))

          (func (export "yield-first") (param $which i32) (result i32)
            (local $plain i32) (local $first i32) (local $status i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $plain (call $started (call $yield-plain (i32.const 0))))
            (local.set $first
              (call $started (call $yield-first (local.get $which) (i32.const 4))))
            (local.set $status (call $cancel-async (local.get $first)))
            (call $subtask.drop (local.get $first))
            (if (i32.ne (call $wait-for (local.get $plain)) (i32.const 2 (; RETURNED ;)))
              (then unreachable))
            (if (i32.ne (i32.load (i32.const 0)) (i32.const 7))
              (then unreachable))
            (call $subtask.drop (local.get $plain))
            (local.get $status))

          (func (export "suspend-not-woken") (param $which i32) (result i32)
            (local $sub i32) (local $state i32)
            (local.set $sub
              (call $started (call $suspend-held (local.get $which) (i32.const 0))))
            (if (i32.ne (call $cancel-async (local.get $sub)) (i32.const -1 (; BLOCKED ;)))
              (then unreachable))
            (call $resume-main)
            (local.set $state (call $wait-for (local.get $sub)))
            (call $subtask.drop (local.get $sub))
            (local.get $state))

          (func (export "never-takes") (result i32)
            (local $sub i32) (local $state i32)
            (i32.store (i32.const 0) (i32.const 0))
            (local.set $sub (call $started (call $never-takes (i32.const 0))))
            (local.set $state
              (call $resolution (local.get $sub) (call $cancel-async (local.get $sub))))
            (if (i32.ne (i32.load (i32.const 0)) (i32.const 7))
              (then unreachable))
            (local.get $state)))
        (core instance $dm (instantiate $DM (with "" (instance
          (export "mem" (memory $memory "mem"))
          (export "cancel-async" (func $cancel-async))
          (export "cancel-sync" (func $cancel-sync))
          (export "subtask.drop" (func $subtask.drop))
          (export "waitable.join" (func $waitable.join))
          (export "waitable-set.new" (func $waitable-set.new))
          (export "waitable-set.drop" (func $waitable-set.drop))
          (export "waitable-set.wait" (func $waitable-set.wait))
          (export "pending" (func $pending'))
          (export "wait-woken" (func $wait-woken'))
          (export "event-waiter" (func $event-waiter'))
          (export "wait-first" (func $wait-first'))
          (export "wait-ready" (func $wait-ready'))
          (export "wait-later" (func $wait-later'))
          (export "drop-reader" (func $drop-reader'))
          (export "future.new" (func $future.new))
          (export "future.write" (func $future.write))
          (export "future.drop-readable" (func $future.drop-readable))
          (export "task.return" (func $task.return))
          (export "yield-plain" (func $yield-plain'))
          (export "yield-first" (func $yield-first'))
          (export "suspend-held" (func $suspend-held'))
          (export "resume-main" (func $resume-main'))
          (export "never-takes" (func $never-takes'))))))
        (func (export "pending") async (param "w" u32) (result u32)
          (canon lift (core func $dm "pending")))
        (func (export "wait-woken-async") async (result u32)
          (canon lift (core func $dm "wait-woken-async")))
        (func (export "wait-woken-sync") async (result u32)
          (canon lift (core func $dm "wait-woken-sync")))
        (func (export "wait-first") async (param "sync" u32) (result u32)
          (canon lift (core func $dm "wait-first")))
        (func (export "wait-ready") async (result u32)
          (canon lift (core func $dm "wait-ready")))
        (func (export "cancel-later") async (param "w" u32) (result u32)
          (canon lift (core func $dm "cancel-later") async
            (callback (core func $dm "cancel-later-cb"))))
        (func (export "trigger") (canon lift (core func $dm "trigger")))
        (func (export "finish") (result u32) (canon lift (core func $dm "finish")))
        (func (export "yield-first") async (param "w" u32) (result u32)
          (canon lift (core func $dm "yield-first")))
        (func (export "suspend-not-woken") async (param "w" u32) (result u32)
          (canon lift (core func $dm "suspend-not-woken")))
        (func (export "never-takes") async (result u32)
          (canon lift (core func $dm "never-takes"))))

      (instance $c (instantiate $C))
      (instance $d (instantiate $D
        (with "pending" (func $c "pending"))
        (with "wait-woken" (func $c "wait-woken"))
        (with "event-waiter" (func $c "event-waiter"))
        (with "wait-first" (func $c "wait-first"))
        (with "wait-ready" (func $c "wait-ready"))
        (with "wait-later" (func $c "wait-later"))
        (with "drop-reader" (func $c "drop-reader"))
        (with "yield-plain" (func $c "yield-plain"))
        (with "yield-first" (func $c "yield-first"))
        (with "suspend-held" (func $c "suspend-held"))
        (with "resume-main" (func $c "resume-main"))
        (with "never-takes" (func $c "never-takes"))))
      (func (export "pending") (alias export $d "pending"))
      (func (export "wait-woken-async") (alias export $d "wait-woken-async"))
      (func (export "wait-woken-sync") (alias export $d "wait-woken-sync"))
      (func (export "wait-first") (alias export $d "wait-first"))
      (func (export "wait-ready") (alias export $d "wait-ready"))
      (func (export "cancel-later") (alias export $d "cancel-later"))
      (func (export "trigger") (alias export $d "trigger"))
      (func (export "finish") (alias export $d "finish"))
      (func (export "yield-first") (alias export $d "yield-first"))
      (func (export "suspend-not-woken") (alias export $d "suspend-not-woken"))
      (func (export "never-takes") (alias export $d "never-takes")))
    "#
);

/// The kinds of built-in that carry the `cancellable` immediate, in
/// the order [`cancellable`] reports them.
const KINDS: [&str; 8] = [
    "waitable-set.wait",
    "waitable-set.poll",
    "thread.yield",
    "thread.suspend",
    "thread.suspend-then-resume",
    "thread.yield-then-resume",
    "thread.suspend-then-promote",
    "thread.yield-then-promote",
];

/// `binary` with the `cancellable` immediate set on the first built-in
/// of each kind in [`KINDS`] it defines, in any of its components.
///
/// Each of these built-ins encodes as its one-byte code followed by
/// the byte that carries the immediate, which a toolchain that no
/// longer spells the immediate leaves zero. The byte is set to one,
/// which is how Wasmtime 49 reads a cancellable built-in. Every kind
/// must be found, so a fixture that stops defining one fails here
/// rather than testing a plain built-in.
fn cancellable(binary: &[u8]) -> Vec<u8> {
    let mut patched = binary.to_vec();
    let mut found = [false; KINDS.len()];
    for payload in Parser::new(0).parse_all(binary) {
        let Payload::ComponentCanonicalSection(section) = payload.expect("the fixture parses")
        else {
            continue;
        };
        for entry in section.into_iter_with_offsets() {
            let (offset, function) = entry.expect("the canonical function parses");
            let kind = match function {
                CanonicalFunction::WaitableSetWait { .. } => 0,
                CanonicalFunction::WaitableSetPoll { .. } => 1,
                CanonicalFunction::ThreadYield { .. } => 2,
                CanonicalFunction::ThreadSuspend { .. } => 3,
                CanonicalFunction::ThreadSuspendThenResume { .. } => 4,
                CanonicalFunction::ThreadYieldThenResume { .. } => 5,
                CanonicalFunction::ThreadSuspendThenPromote { .. } => 6,
                CanonicalFunction::ThreadYieldThenPromote { .. } => 7,
                _ => continue,
            };
            if found[kind] {
                continue;
            }
            found[kind] = true;
            let immediate = usize::try_from(offset).expect("an offset fits") + 1;
            assert_eq!(
                patched[immediate], 0,
                "the {} built-in was assembled without the immediate",
                KINDS[kind]
            );
            patched[immediate] = 1;
        }
    }
    for (kind, found) in KINDS.iter().zip(found) {
        assert!(found, "the fixture defines no {kind} built-in");
    }
    patched
}

/// Instantiate `binary` in a store of its own, with the engine's
/// suspend provider on when `provider` says so and off otherwise. The
/// engine accepts the stackful form of `canon lift async`, which the
/// callee is lifted in, the thread built-ins, and the `async` option
/// of `subtask.cancel`: Wasmtime keeps each behind a flag.
async fn instantiate(binary: &[u8], provider: bool) -> (Store<()>, Instance) {
    let mut config = EngineConfig::new();
    config
        .wasm_component_model_async_stackful(true)
        .wasm_component_model_threading(true)
        .wasm_component_model_more_async_builtins(true)
        .suspend_provider(provider);
    let engine = Engine::with_backend(crate::test_backend::backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    assert_eq!(
        engine.suspend_provider() != SuspendProviderKind::None,
        provider,
        "the rules are proved with the provider each test names"
    );
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

/// Call the caller's export `name` with `args` in a fresh instance of
/// the cancellable [`CANCELLABLE`], under the provider, and answer the
/// `u32` it returned.
async fn resolve(name: &str, args: &[Val]) -> u32 {
    let (mut store, instance) = instantiate(&cancellable(CANCELLABLE), true).await;
    let func = instance.get_func(name).expect("the export is declared");
    match func
        .call(&mut store, args)
        .await
        .unwrap_or_else(|error| panic!("{name} {args:?} failed: {}", chain(&error)))
        .into_vec()
        .as_slice()
    {
        [Val::U32(state)] => *state,
        other => panic!("{name} answered {other:?} rather than one u32"),
    }
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

#[wcmp_macros::test]
async fn it_answers_the_cancelled_result_at_once_from_each_cancellable_built_in() {
    for (which, built_in) in PENDING_BUILT_INS {
        assert_eq!(
            resolve("pending", &[Val::U32(which)]).await,
            CANCELLED_BEFORE_RETURNED,
            "a cancellable {built_in} takes the pending request, so the callee can confirm"
        );
    }
}

#[wcmp_macros::test]
async fn it_wakes_a_blocked_cancellable_wait_with_the_task_cancelled_event() {
    assert_eq!(
        resolve("wait-woken-async", &[]).await,
        CANCELLED_BEFORE_RETURNED,
        "the cancelled wait answers the task-cancelled event, and the callee confirms"
    );
    assert_eq!(
        resolve("wait-woken-sync", &[]).await,
        CANCELLED_BEFORE_RETURNED,
        "a synchronous cancel ends the wait the same way"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_woken_cancellable_wait_ahead_of_a_wait_that_was_ready_first() {
    for (sync, cancel) in [(0, "an asynchronous"), (1, "a synchronous")] {
        assert_eq!(
            resolve("wait-first", &[Val::U32(sync)]).await,
            CANCELLED_BEFORE_RETURNED,
            "{cancel} cancel wakes the wait and runs it before the waiter an event made ready"
        );
    }
}

#[wcmp_macros::test]
async fn it_takes_the_request_before_an_event_the_set_holds_and_leaves_the_event() {
    for (which, built_in) in [(8, "waitable-set.wait"), (9, "waitable-set.poll")] {
        assert_eq!(
            resolve("pending", &[Val::U32(which)]).await,
            CANCELLED_BEFORE_RETURNED,
            "a cancellable {built_in} answers the task-cancelled event first, and the set keeps its event"
        );
    }
}

#[wcmp_macros::test]
async fn it_does_not_wake_a_cancellable_wait_an_event_already_made_ready() {
    assert_eq!(
        resolve("wait-ready", &[]).await,
        CANCELLED_BEFORE_RETURNED,
        "the wait runs in its turn behind the waiter made ready before it, takes the request \
         first, and the set keeps its event"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_ready_cancellable_yield_first_and_answers_one() {
    for (which, built_in) in [(0, "thread.yield"), (1, "thread.yield-then-promote")] {
        assert_eq!(
            resolve("yield-first", &[Val::U32(which)]).await,
            CANCELLED_BEFORE_RETURNED,
            "the cancel runs the cancellable {built_in} ahead of a yield that was ready first"
        );
    }
}

#[wcmp_macros::test]
async fn it_does_not_wake_a_cancellable_suspension_and_answers_one_once_resumed() {
    for (which, built_in) in [(0, "thread.suspend"), (1, "thread.suspend-then-promote")] {
        assert_eq!(
            resolve("suspend-not-woken", &[Val::U32(which)]).await,
            CANCELLED_BEFORE_RETURNED,
            "the cancel leaves the cancellable {built_in} suspended, and a resume finds the request"
        );
    }
}

#[wcmp_macros::test]
async fn it_never_takes_a_request_in_a_built_in_without_the_immediate() {
    assert_eq!(
        resolve("never-takes", &[]).await,
        RETURNED,
        "no built-in without the immediate takes the request, so the callee returns"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_cancellable_wait_whose_request_lies_in_a_frame_below_when_the_provider_is_off()
{
    // With no provider the callee's wait blocks in a nested turn above
    // the caller's frame. The caller would make the request once the
    // callee gave control back, which only a stack switch could do, so
    // the turn runs out of work and the wait fails with the
    // stack-switch cause rather than waiting for a request that cannot
    // come.
    let (mut store, instance) = instantiate(&cancellable(CANCELLABLE), false).await;
    let func = instance
        .get_func("wait-woken-async")
        .expect("the export is declared");

    let error = match func.call(&mut store, &[]).await {
        Err(error) => chain(&error),
        Ok(values) => panic!("the wait returned {values:?} rather than failing"),
    };

    let cause = SchedulerCause::StackSwitchNeeded.to_string();
    assert!(
        error.contains(&cause),
        "the wait must fail with `{cause}`, got: {error}"
    );
}

/// Run `wait-later` with the provider off, and answer what the cancel
/// answered and the state the subtask resolved to.
///
/// `cancel-later` starts `wait-later`, whose event loop waits for a
/// write, and waits in its own loop for another. The two calls are
/// queued together, so `trigger` runs once `cancel-later` waits: it
/// makes `wait-later`'s loop ready, then `cancel-later`'s. The
/// callback of `wait-later` runs first and blocks in the cancellable
/// built-in `which` selects, on the real stack, and the turn that
/// block runs gives the callback of `cancel-later` its go. That
/// callback cancels `wait-later` from inside the turn, and returns
/// what the cancel answered.
async fn cancel_later(which: u32) -> (u32, u32) {
    let (mut store, instance) = instantiate(&cancellable(CANCELLABLE), false).await;
    let cancel = instance
        .get_func("cancel-later")
        .expect("the export is declared");
    let trigger = instance
        .get_func("trigger")
        .expect("the export is declared");
    let args = [Val::U32(which)];

    let (cancelled, triggered) = store
        .run_concurrent(async |accessor| {
            let mut cancelled = Box::pin(cancel.call_concurrent(accessor, &args));
            let mut triggered = Box::pin(trigger.call_concurrent(accessor, &[]));
            let mut cancelled_done: Option<Result<Box<[Val]>, Error>> = None;
            let mut triggered_done: Option<Result<Box<[Val]>, Error>> = None;
            poll_fn(|context| {
                if cancelled_done.is_none()
                    && let Poll::Ready(value) = cancelled.as_mut().poll(context)
                {
                    cancelled_done = Some(value);
                }
                if triggered_done.is_none()
                    && let Poll::Ready(value) = triggered.as_mut().poll(context)
                {
                    triggered_done = Some(value);
                }
                if cancelled_done.is_some() && triggered_done.is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            (
                cancelled_done.expect("cancel-later resolved"),
                triggered_done.expect("trigger resolved"),
            )
        })
        .await
        .expect("the driver returns");

    triggered.unwrap_or_else(|error| panic!("trigger failed: {}", chain(&error)));
    let cancelled =
        cancelled.unwrap_or_else(|error| panic!("cancel-later failed: {}", chain(&error)));
    let [Val::U32(answer)] = cancelled.as_ref() else {
        panic!("cancel-later answered {cancelled:?} rather than one u32");
    };
    let finish = instance.get_func("finish").expect("the export is declared");
    let finished = finish
        .call(&mut store, &[])
        .await
        .unwrap_or_else(|error| panic!("finish failed: {}", chain(&error)))
        .into_vec();
    let [Val::U32(state)] = finished.as_slice() else {
        panic!("finish answered {finished:?} rather than one u32");
    };
    (*answer, *state)
}

#[wcmp_macros::test]
async fn it_wakes_a_cancellable_wait_with_a_request_made_inside_its_turn_when_the_provider_is_off()
{
    assert_eq!(
        cancel_later(0).await,
        (BLOCKED, CANCELLED_BEFORE_RETURNED),
        "the cancel cannot run the wait, whose frame lies below it, so it answers BLOCKED; the \
         wait then answers the task-cancelled event and the callee confirms"
    );
}

#[wcmp_macros::test]
async fn it_answers_one_from_a_cancellable_yield_on_the_real_stack_when_the_provider_is_off() {
    for (which, built_in) in [(1, "thread.yield"), (2, "thread.yield-then-promote")] {
        assert_eq!(
            cancel_later(which).await,
            (BLOCKED, CANCELLED_BEFORE_RETURNED),
            "the cancellable {built_in} takes the request made inside the turn it gave way to, \
             answers 1, and the callee confirms"
        );
    }
}

#[path = "support/backend.rs"]
mod test_backend;
