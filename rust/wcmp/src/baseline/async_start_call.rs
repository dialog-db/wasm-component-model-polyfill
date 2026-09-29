//! Baseline tests for the prepare-and-start protocol of the fused
//! adapters, on its asynchronous half.
//!
//! An asynchronous lower of another component's export reaches the
//! asynchronous start: the callee runs next, and the caller has the
//! status word when the intrinsic returns rather than the result.
//! The four combinations of lower and lift are the corpus's own, in
//! `wasmtime/async/fused.wast` and `cm/async/cross-abi-calls.wast`.
//!
//! What the tests here cover is what no corpus file reaches from a
//! repository test: that the subtask event of the callee's
//! resolution reaches a caller which already took delivery of the
//! start, with the callee's result already in the caller's memory
//! when the callback runs. A guest callee resolves at its
//! `task.return`, and nothing else fills the subtask's event slot
//! afterwards, so this is the one observation that says the
//! resolution is recorded where the caller is waiting.
//!
//! The failure paths are here for the same reason. A trap or an
//! exception in the callee's first phase unwinds to the lower's
//! trampoline and fails the caller's call; one in its callback
//! reaches no trampoline, because the callee gave way and the caller
//! parked, and fails the driver whose turn ran the callback instead.
//! Either way the callee's side is wound back: its task ends, the
//! instance its thread held exclusively goes back, and the caller's
//! record of the call is cancelled, which gives back the handles it
//! lent. What is left over differs. A caller the trampoline failed
//! is unwound with the call, so the store is as the call found it; a
//! caller that had already parked keeps its task and the set it
//! joined the subtask to, because the failure ends the driver's turn
//! and not the task the driver was waiting on.
//!
//! The lends are the subtask's, which is what makes the cancellation
//! the moment they come back. The success path is here for the same
//! reason: a callee that `task.return`s and keeps running gives the
//! caller its handles back at the delivery of the resolution, which
//! is earlier than that callee's task exit.
//!
//! The other half of the gate is here too: a callee the gate holds
//! reads `STARTING`, and the call is served rather than refused
//! once the gate opens. Nothing traps for reentrance — the gate is
//! the only serialization — so the whole of a held call is the wait
//! and the `RETURNED` that ends it.

#![cfg(test)]

use crate::internal::FuncInternal;
use crate::resource::HandleKind;
use crate::store::StoreInternalExt;
use crate::{Component, Engine, Error, Instance, Linker, Store, TaskCause, Val};
use wcmp_macros::component;

/// A caller that reads `STARTED`, joins the subtask to a waitable
/// set, and parks; and a callee that parks before it returns, so the
/// caller can only learn the result from the subtask event.
///
/// The callee's first call stores its argument and gives way. Its
/// callback doubles the argument and calls `task.return`, which runs
/// the return function of the prepared call and writes the result
/// into the caller's memory at the return pointer the caller passed.
/// The caller's callback then traps unless the event is the subtask
/// event carrying `RETURNED` (2), and adds one to what it finds at
/// that pointer, so the result the host reads says both that the
/// event arrived and that the memory was already written when it
/// did. It then drops the subtask and the set, which the delivered
/// resolution is what allows, so nothing of the call is left in the
/// store when the host's call returns.
const PARKS_THEN_RETURNS: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (global $x (mut i32) (i32.const 0))
          (func (export "answer") (param i32) (result i32)
            (global.set $x (local.get 0))
            (i32.const 1))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (call $task-return (i32.mul (global.get $x) (i32.const 2)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $set-drop (canon waitable-set.drop))
        (core func $join (canon waitable.join))
        (core func $subtask-drop (canon subtask.drop))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (global $set (mut i32) (i32.const 0))
          (func (export "run") (param i32) (result i32)
            (local $status i32)
            (local.set $status (call $answer (local.get 0) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
              (then unreachable))
            (global.set $set (call $set-new))
            (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
            (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 1)) (then unreachable))
            (if (i32.ne (local.get 2) (i32.const 2)) (then unreachable))
            (call $join (local.get 1) (i32.const 0))
            (call $subtask-drop (local.get 1))
            (call $set-drop (global.get $set))
            (call $task-return (i32.add (i32.load (i32.const 8)) (i32.const 1)))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.drop" (func $set-drop))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb")))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// The same shape with a callee that returns before its core
/// function does, so the call resolves while the caller's start
/// intrinsic is still on the stack. The caller traps unless the
/// status word is `RETURNED` (2) with no index above it, and reads
/// the result out of its own memory, which the crossing wrote before
/// the lower returned.
const RESOLVES_AT_ONCE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32)
            (call $task-return (i32.mul (local.get 0) (i32.const 2)))
            (i32.const 0)))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (if (i32.ne (call $answer (local.get 0) (i32.const 8)) (i32.const 2))
              (then unreachable))
            (i32.add (i32.load (i32.const 8)) (i32.const 1))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// A synchronously lifted callee under an asynchronous lower, with a
/// `post-return`. The results cross as the callee's core function
/// returns, so the caller reads `RETURNED` too, and the callee's
/// `post-return` records that it ran in the callee's own memory,
/// which the callee's second export reports.
const SYNCHRONOUS_CALLEE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core module $m
          (memory (export "mem") 1)
          (func (export "answer") (param i32) (result i32)
            (i32.mul (local.get 0) (i32.const 2)))
          (func (export "post") (param i32)
            (i32.store (i32.const 4) (local.get 0)))
          (func (export "ran") (result i32)
            (i32.load (i32.const 4))))
        (core instance $i (instantiate $m))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") (post-return (core func $i "post"))))
        (func (export "ran") (result u32)
          (canon lift (core func $i "ran"))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (if (i32.ne (call $answer (local.get 0) (i32.const 8)) (i32.const 2))
              (then unreachable))
            (i32.add (i32.load (i32.const 8)) (i32.const 1))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run"))
      (export "ran" (func $a "ran")))
    "#
);

/// A synchronously lifted callee the entry gate holds, under an
/// asynchronous lower.
///
/// The callee raises its own instance's backpressure through a
/// synchronous export, so the start intrinsic finds the gate shut
/// and the lower answers `STARTING` — the callee has not read its
/// parameters, and the caller traps unless the word says so. The
/// caller then joins the subtask to a set, lowers the backpressure
/// again through a second synchronous export, which is sync-typed
/// and so ignores the gate, and waits. The gate opens, the callee
/// runs, and its result crosses into the caller's memory, so the
/// caller's callback traps unless the event carries `RETURNED` and
/// the memory at the return pointer is already written.
const HELD_AT_THE_GATE: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $inc (canon backpressure.inc))
        (core func $dec (canon backpressure.dec))
        (core module $m
          (import "" "backpressure.inc" (func $inc))
          (import "" "backpressure.dec" (func $dec))
          (func (export "answer") (param i32) (result i32)
            (i32.mul (local.get 0) (i32.const 2)))
          (func (export "block") (call $inc))
          (func (export "unblock") (call $dec)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "backpressure.inc" (func $inc))
            (export "backpressure.dec" (func $dec))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer")))
        (func (export "block") (canon lift (core func $i "block")))
        (func (export "unblock") (canon lift (core func $i "unblock"))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (import "block" (func $block))
        (import "unblock" (func $unblock))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core func $block' (canon lower (func $block)))
        (core func $unblock' (canon lower (func $unblock)))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $set-drop (canon waitable-set.drop))
        (core func $join (canon waitable.join))
        (core func $subtask-drop (canon subtask.drop))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "block" (func $block))
          (import "" "unblock" (func $unblock))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (global $set (mut i32) (i32.const 0))
          (global $sub (mut i32) (i32.const 0))
          (func (export "run") (param i32) (result i32)
            (local $status i32)
            (call $block)
            (local.set $status (call $answer (local.get 0) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 0))
              (then unreachable))
            (global.set $sub (i32.shr_u (local.get $status) (i32.const 4)))
            (global.set $set (call $set-new))
            (call $join (global.get $sub) (global.get $set))
            (call $unblock)
            (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 1)) (then unreachable))
            (if (i32.ne (local.get 1) (global.get $sub)) (then unreachable))
            (if (i32.ne (local.get 2) (i32.const 2)) (then unreachable))
            (call $join (local.get 1) (i32.const 0))
            (call $subtask-drop (local.get 1))
            (call $set-drop (global.get $set))
            (call $task-return (i32.add (i32.load (i32.const 8)) (i32.const 1)))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))
          (export "block" (func $block'))
          (export "unblock" (func $unblock'))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.drop" (func $set-drop))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb")))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller
        (with "answer" (func $a "answer"))
        (with "block" (func $a "block"))
        (with "unblock" (func $a "unblock"))))
      (export "run" (func $b "run")))
    "#
);

/// A callee whose core function traps, under an asynchronous lower.
/// The trap unwinds through the start item to the trampoline and
/// fails the caller's call, as it does for the synchronous start.
const TRAPS: &[u8] = component!(
    r#"
    (component
      (component $callee
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable)
          (func (export "answer") (param i32) (result i32) unreachable))
        (core instance $i (instantiate $m
          (with "" (instance (export "task.return" (func $task-return))))))
        (func (export "answer") async (param "x" u32) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "answer" (func $answer async (param "x" u32) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (func (export "run") (param i32) (result i32)
            (call $answer (local.get 0) (i32.const 8))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "answer" (func $lowered))))))
        (func (export "run") (param "x" u32) (result u32)
          (canon lift (core func $i "run"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller (with "answer" (func $a "answer"))))
      (export "run" (func $b "run")))
    "#
);

/// A callee that gives way in its first phase and throws an
/// exception it does not catch in its callback, under an
/// asynchronous lower, with a caller that parks on the subtask and
/// lends it a borrow.
///
/// Nothing of the call is on the stack by the time the callback
/// runs: the callee gave way, so its callback item waits on the
/// low-priority queue; the caller read `STARTED`, joined the subtask
/// to a waitable set, and returned the wait word, so its own task
/// parked too. The driver that runs the callback item next is
/// therefore the host's call into the caller, and the exception the
/// callback throws is that driver's failure.
///
/// The argument is a borrow, so the failure has a lend to give back.
/// The caller mints an owning handle of the callee's resource type
/// and keeps the index in a global; `drop-now` drops that handle
/// where it stands, which traps while the borrow is still lent and
/// succeeds once the lend is undone. The failure poisons the store,
/// so a test reads the lend from the caller's table rather than
/// calling it.
const THROWS_AFTER_YIELD: &[u8] = component!(
    r#"
    (component
      (component $callee
        (type $t' (resource (rep i32)))
        (core func $new (canon resource.new $t'))
        (core module $m
          (import "" "new" (func $new (param i32) (result i32)))
          (tag $e)
          (func (export "cb") (param i32 i32 i32) (result i32)
            (throw $e))
          (func (export "answer") (param i32) (result i32)
            (i32.const 1))
          (func (export "make") (param i32) (result i32)
            (call $new (local.get 0))))
        (core instance $i (instantiate $m (with "" (instance
          (export "new" (func $new))))))
        (export $t "thing" (type $t'))
        (func (export "make") (param "rep" u32) (result (own $t))
          (canon lift (core func $i "make")))
        (func (export "answer") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "thing" (type $t (sub resource)))
        (import "make" (func $make (param "rep" u32) (result (own $t))))
        (import "answer" (func $answer async (param "x" (borrow $t)) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $make (canon lower (func $make)))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core func $drop-thing (canon resource.drop $t))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "make" (func $make (param i32) (result i32)))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "drop-thing" (func $drop-thing (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (global $handle (mut i32) (i32.const 0))
          (global $set (mut i32) (i32.const 0))
          (func (export "run") (param i32) (result i32)
            (local $status i32)
            (global.set $handle (call $make (local.get 0)))
            (local.set $status (call $answer (global.get $handle) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
              (then unreachable))
            (global.set $set (call $set-new))
            (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
            (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "drop-now") (call $drop-thing (global.get $handle)))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "make" (func $make))
          (export "answer" (func $lowered))
          (export "drop-thing" (func $drop-thing))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb"))))
        (func (export "drop-now") (canon lift (core func $i "drop-now"))))
      (instance $a (instantiate $callee))
      (alias export $a "thing" (type $t))
      (instance $b (instantiate $caller
        (with "thing" (type $t))
        (with "make" (func $a "make"))
        (with "answer" (func $a "answer"))))
      (export "run" (func $b "run"))
      (export "drop-now" (func $b "drop-now")))
    "#
);

/// A callee that `task.return`s and keeps running, with a caller
/// that lends it a borrow of its own owning handle.
///
/// This is where the record a lend lives on can be read off the
/// store. The callee gives way in its first phase, so the caller
/// reads `STARTED`, joins the subtask to a set and parks. The
/// callee's callback then calls `task.return`, which resolves the
/// call, and waits on a fresh set no turn ever fills, so the
/// callee's task is still in the store — and still holds the
/// instance — for the rest of the test.
///
/// The caller's callback takes the subtask event, which delivers the
/// resolution, and drops the owning handle it lent from. That drop
/// stands only if the lend went on the subtask: a lend on the
/// callee's task would still be outstanding, because that task has
/// not exited and will not. `run` therefore answers 43 — the
/// callee's 42 plus one — only when the delivery is what gave the
/// handle back.
///
/// `drop-early` is the negative control on the same component. It
/// drops the owning handle on the instruction after the lower
/// returns, with the resolution undelivered, and traps.
const RETURNS_AND_KEEPS_RUNNING: &[u8] = component!(
    r#"
    (component
      (component $callee
        (type $t' (resource (rep i32)))
        (core func $new (canon resource.new $t'))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core module $m
          (import "" "new" (func $new (param i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (func (export "make") (param i32) (result i32)
            (call $new (local.get 0)))
          (func (export "answer") (param i32) (result i32)
            (i32.const 1))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (call $task-return (i32.const 42))
            (i32.or (i32.shl (call $set-new) (i32.const 4)) (i32.const 2))))
        (core instance $i (instantiate $m (with "" (instance
          (export "new" (func $new))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))))))
        (export $t "thing" (type $t'))
        (func (export "make") (param "rep" u32) (result (own $t))
          (canon lift (core func $i "make")))
        (func (export "answer") async (param "x" (borrow $t)) (result u32)
          (canon lift (core func $i "answer") async (callback (core func $i "cb")))))
      (component $caller
        (import "thing" (type $t (sub resource)))
        (import "make" (func $make (param "rep" u32) (result (own $t))))
        (import "answer" (func $answer async (param "x" (borrow $t)) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $make (canon lower (func $make)))
        (core func $lowered
          (canon lower (func $answer) async (memory (core memory $libc "mem"))))
        (core func $drop-thing (canon resource.drop $t))
        (core func $task-return (canon task.return (result u32)))
        (core func $set-new (canon waitable-set.new))
        (core func $set-drop (canon waitable-set.drop))
        (core func $join (canon waitable.join))
        (core func $subtask-drop (canon subtask.drop))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "make" (func $make (param i32) (result i32)))
          (import "" "answer" (func $answer (param i32 i32) (result i32)))
          (import "" "drop-thing" (func $drop-thing (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (global $handle (mut i32) (i32.const 0))
          (global $set (mut i32) (i32.const 0))
          (func (export "run") (param i32) (result i32)
            (local $status i32)
            (global.set $handle (call $make (local.get 0)))
            (local.set $status (call $answer (global.get $handle) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
              (then unreachable))
            (global.set $set (call $set-new))
            (call $join (i32.shr_u (local.get $status) (i32.const 4)) (global.get $set))
            (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
          (func (export "drop-early") (param i32) (result i32)
            (local $status i32)
            (global.set $handle (call $make (local.get 0)))
            (local.set $status (call $answer (global.get $handle) (i32.const 8)))
            (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
              (then unreachable))
            (call $drop-thing (global.get $handle))
            (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32)
            (if (i32.ne (local.get 0) (i32.const 1)) (then unreachable))
            (if (i32.ne (local.get 2) (i32.const 2)) (then unreachable))
            (call $join (local.get 1) (i32.const 0))
            (call $subtask-drop (local.get 1))
            (call $set-drop (global.get $set))
            (call $drop-thing (global.get $handle))
            (call $task-return (i32.add (i32.load (i32.const 8)) (i32.const 1)))
            (i32.const 0)))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "make" (func $make))
          (export "answer" (func $lowered))
          (export "drop-thing" (func $drop-thing))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.drop" (func $set-drop))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async (callback (core func $i "cb"))))
        (func (export "drop-early") async (param "x" u32) (result u32)
          (canon lift (core func $i "drop-early") async (callback (core func $i "cb")))))
      (instance $a (instantiate $callee))
      (alias export $a "thing" (type $t))
      (instance $b (instantiate $caller
        (with "thing" (type $t))
        (with "make" (func $a "make"))
        (with "answer" (func $a "answer"))))
      (export "run" (func $b "run"))
      (export "drop-early" (func $b "drop-early")))
    "#
);

async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// The whole message of an error and everything under it, on one
/// line.
fn chain(error: &crate::Error) -> String {
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

/// How many subtask records the store holds.
fn subtask_count(store: &Store<()>) -> usize {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .subtask_count()
}

/// Whether any component instance of the store is held exclusively
/// by a thread.
fn any_instance_is_held(store: &Store<()>) -> bool {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .instances()
        .iter()
        .any(|record| record.exclusive_thread.is_some())
}

/// Take one turn of the store's scheduler and name what it achieved.
///
/// The turn is the whole of a driver's poll, without a call to give
/// it a condition, which is what makes it the way to pin what a
/// driver would find. Its answer is named rather than returned,
/// because the enumeration the store answers with is not part of the
/// crate's public surface.
fn turn(store: &mut Store<()>) -> String {
    let outcome = store
        .internal()
        .turn(core::task::Waker::noop())
        .expect("the turn itself does not fail");
    format!("{outcome:?}")
}

#[wcmp_macros::test]
async fn it_delivers_the_subtask_event_with_the_result_already_in_the_callers_memory() {
    // The caller took `STARTED` and went back to waiting, so the
    // resolution has to fill the subtask's event slot where it
    // happens — at the callee's `task.return` — for the caller ever
    // to hear of it. The result crosses first, at that same moment,
    // so the caller's callback finds its memory written before the
    // event reaches it: it doubles in the callee, one is added in
    // the caller's callback, and 43 says both halves happened in
    // that order.
    let (mut store, instance) = instantiate(PARKS_THEN_RETURNS).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);
    assert_eq!(task_count(&store), 0, "both tasks left the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_answers_with_the_returned_status_when_the_callee_resolves_at_once() {
    // The gate is open and the callee returns its result inside the
    // start item, so the call resolves before the lower returns: the
    // status word is `RETURNED` with no index, the caller is given
    // no entry to wait on, and the result is already at the return
    // pointer the caller passed.
    let (mut store, instance) = instantiate(RESOLVES_AT_ONCE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);
    assert_eq!(subtask_count(&store), 0, "no subtask is left behind");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_crosses_the_results_of_a_synchronously_lifted_callee_and_runs_its_post_return() {
    // A synchronously lifted callee has no `task.return`: its
    // results cross as its core function returns, and its
    // `post-return` runs afterwards, inside its own task. The caller
    // therefore reads `RETURNED` too, and the callee's own record of
    // its `post-return` says the second half ran.
    let (mut store, instance) = instantiate(SYNCHRONOUS_CALLEE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);

    let ran = instance.get_func("ran").expect("the callee's witness");
    let witness = ran.call(&mut store, &[]).await.expect("the call returns");
    assert_eq!(
        witness.as_ref(),
        &[Val::U32(42)],
        "the `post-return` ran with the flat result the core function returned"
    );
    assert_eq!(subtask_count(&store), 0, "no subtask is left behind");
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_runs_a_held_callee_when_the_gate_opens_and_delivers_returned() {
    // The gate is the only serialization there is, and a reentrant
    // or back-pressured call meets it rather than a trap. The caller
    // reads `STARTING`, which says the callee has not run at all,
    // opens the gate again through a sync-typed export that ignores
    // it, and waits. The callee then runs and resolves, and 43 says
    // the caller's callback found `RETURNED` with the result already
    // in its memory.
    let (mut store, instance) = instantiate(HELD_AT_THE_GATE).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the call returns");
    assert_eq!(result.as_ref(), &[Val::U32(43)]);
    assert_eq!(task_count(&store), 0, "both tasks left the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert_eq!(
        store.internal().scheduler().waiting_at_gate(),
        0,
        "nothing is left waiting at the gate"
    );
    assert!(!any_instance_is_held(&store));
}

#[wcmp_macros::test]
async fn it_fails_the_callers_call_when_the_callee_traps() {
    // The trap unwinds through the start item into the trampoline,
    // which is on the stack while the switch slot runs, and fails
    // the caller's call with the message the synchronous baseline
    // gives the same trap. Nothing is left behind for the next call
    // to wait behind.
    let (mut store, instance) = instantiate(TRAPS).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the callee's trap fails the caller's call");
    let message = chain(&err);
    assert!(
        message.contains("unreachable"),
        "expected the baseline's trap message, got {message}"
    );
    assert_eq!(task_count(&store), 0, "neither task is left in the store");
    assert_eq!(subtask_count(&store), 0, "the subtask left the store");
    assert!(
        !any_instance_is_held(&store),
        "the callee's exclusive thread is released by the failure"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_driver_that_ran_the_callback_when_the_callee_throws_after_it_gave_way() {
    // The callee gave way, so its callback runs in a later turn of
    // the driver rather than under the lower's trampoline. The
    // exception the callback throws is not caught in the callee, so
    // the runtime layer hands it to the polyfill as the failure of
    // the call into the callback, and the item carries it out
    // unchanged: it ends the turn, and the driver of the host's call
    // fails with the message the synchronous baseline gives the same
    // throw. The callee's task and the caller's record of the call
    // go with it, so the instance the callback held exclusively is
    // back and the handles the caller lent are given back. The lends
    // are on the subtask, so it is the cancellation of the call that
    // gives them back rather than the callee's task ending.
    let (mut store, instance) = instantiate(THROWS_AFTER_YIELD).await;
    let run = instance.get_func("run").expect("the caller's export");
    let err = run
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("the callee's exception fails the driver's turn");
    let message = chain(&err);
    assert!(
        message.contains("thrown Wasm exception"),
        "expected the baseline's message for an uncaught exception, got {message}"
    );
    // One record is left, and it is the caller's. The sibling above
    // ends at zero because the trampoline unwinds the caller's call
    // with the failure; here the caller had already parked, so the
    // failure is the driver's rather than the caller's task's. The
    // driver carries the error out of the turn and cleans up nothing
    // of the task it was waiting on, and the asynchronous call it
    // was driving adds no cleanup of its own — the parked task, its
    // implicit thread and the waitable set it joined the subtask to
    // stay where they are. Only the callee's task record leaves.
    assert_eq!(
        task_count(&store),
        1,
        "the caller's parked task is the one record the failure leaves"
    );
    assert_eq!(
        subtask_count(&store),
        0,
        "the subtask of the failed call left the store"
    );
    assert!(
        !any_instance_is_held(&store),
        "the exclusive thread the callback took is released by the failure"
    );
    // Nothing is left for a driver to run. The failure is a trap, and
    // a trap poisons the store and discards every item it holds: the
    // callee's callback item — the one the failure came out of — and
    // the caller's own callback, held for an event on the set it
    // joined the subtask to. The caller's task record stays, as the
    // count above says, and nothing of it will run again.
    assert_eq!(
        store.internal().scheduler().held_callbacks(),
        0,
        "the trap discarded the caller's held callback"
    );
    assert_eq!(
        store.internal().scheduler().queued_items(),
        0,
        "the store holds no item at all"
    );

    // The turn the next driver takes finds nothing ready and goes
    // idle rather than running an item of a poisoned store.
    assert_eq!(turn(&mut store), "Idle");

    // And the borrow the caller lent for the call is back. The
    // failure is a trap, and a trap poisons the store, so the caller
    // cannot drop the handle it lent from any more: the lend is read
    // from the caller's table instead, where that handle is the first
    // entry, at index 1.
    let drop_now = instance
        .get_func("drop-now")
        .expect("the caller's drop export");
    let table = {
        let state = drop_now
            .abi_state()
            .lock()
            .expect("the instance's ABI state");
        state.handle_tables[drop_now.options().instance]
    };
    assert!(
        matches!(
            store
                .internal()
                .tables()
                .lock()
                .expect("handle tables")
                .entry(table, 1),
            Some(HandleKind::Own { lend_count: 0, .. })
        ),
        "the owning handle is no longer lent"
    );
    let refused = drop_now
        .call(&mut store, &[])
        .await
        .expect_err("a poisoned store refuses the call");
    assert!(
        matches!(refused, Error::Task(TaskCause::CannotEnter)),
        "expected the cannot-enter cause, got {refused:?}"
    );
}

#[wcmp_macros::test]
async fn it_gives_a_lent_handle_back_when_the_caller_takes_delivery_of_the_resolution() {
    // The lend a guest-to-guest call records lives on the subtask,
    // so it comes back at the delivery of the resolution rather than
    // at the callee's task exit. The two moments are different here:
    // the callee `task.return`s and then waits on a set no turn ever
    // fills, so its task is still running when the caller takes the
    // subtask event. The caller drops the owning handle it lent from
    // the instruction after that delivery, and 43 is what says the
    // drop stood.
    let (mut store, instance) = instantiate(RETURNS_AND_KEEPS_RUNNING).await;
    let run = instance.get_func("run").expect("the caller's export");
    let result = run
        .call(&mut store, &[Val::U32(7)])
        .await
        .expect("the call returns");
    assert_eq!(
        result.as_ref(),
        &[Val::U32(43)],
        "the caller dropped the handle it lent once the resolution was delivered"
    );
    assert_eq!(
        task_count(&store),
        1,
        "the callee's task is still in the store, so the lend did not come \
         back with its exit"
    );
    assert_eq!(
        subtask_count(&store),
        0,
        "the caller dropped the subtask the delivered resolution let it drop"
    );
}

#[wcmp_macros::test]
async fn it_keeps_a_handle_lent_until_the_resolution_is_delivered() {
    // The negative control on the same component. `drop-early` is
    // `run` with the drop moved to the instruction after the lower
    // returned, where the call has started and nothing has taken its
    // resolution. The lend still stands, so the drop traps, which is
    // what the test above runs past rather than around.
    let (mut store, instance) = instantiate(RETURNS_AND_KEEPS_RUNNING).await;
    let early = instance
        .get_func("drop-early")
        .expect("the caller's early-drop export");
    let err = early
        .call(&mut store, &[Val::U32(7)])
        .await
        .expect_err("the owning handle cannot be dropped while it is lent");
    let message = chain(&err);
    assert!(
        message.contains("cannot remove owned resource while borrowed"),
        "expected the lent-handle trap, got {message}"
    );
}
