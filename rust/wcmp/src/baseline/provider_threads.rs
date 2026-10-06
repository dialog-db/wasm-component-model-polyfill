// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for guest threads that run through the store's
//! suspend provider.
//!
//! Under a provider, each thread entry — a task's core function, a
//! callback, a thread's start function — starts on a stack of its
//! own, and a blocking built-in reaches the guest as the switch
//! module's shim for it. The shim tries the built-in and suspends the
//! thread's stack when it is not ready, and the scheduler resumes the
//! thread once its readiness condition holds. A trampoline that
//! starts a thread starts it as a nested start: the thread runs above
//! the trampoline on a stack of its own, and the trampoline goes on
//! once it suspends or finishes. No suspension runs a nested turn,
//! and the provider stays in the store for the whole of it.
//!
//! Each test states what a store with no provider does instead, and
//! checks that too, so the tests run in every lane: the native engine
//! selects the stack-switching provider on x86_64 Linux and no
//! provider elsewhere, and a browser that ships JavaScript Promise
//! Integration runs every thread through the host-suspension provider.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::concurrency::StoreProvider;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{
    Accessor, Component, Engine, EngineConfig, Error, Func, HostCall, Instance, Linker, Store,
    SuspendProviderKind, Val,
};
use wcmp_macros::component;

/// A stackful export of one component that calls a stackful export
/// of a second through a fused adapter, with a synchronous lower.
///
/// The callee notes `1`, yields, notes `2`, and returns twice its
/// argument. The caller notes `3` once the lower returns, and returns
/// one more than what it got.
const CALLS_A_SECOND_COMPONENT: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (component $callee
        (import "note" (func $note (param "step" u32)))
        (core func $note (canon lower (func $note)))
        (core func $yield (canon thread.yield))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "yield" (func $yield (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "double") (param i32)
            (call $note (i32.const 1))
            (drop (call $yield))
            (call $note (i32.const 2))
            (call $task-return (i32.mul (local.get 0) (i32.const 2)))))
        (core instance $i (instantiate $m (with "" (instance
          (export "note" (func $note))
          (export "yield" (func $yield))
          (export "task.return" (func $task-return))))))
        (func (export "double") async (param "x" u32) (result u32)
          (canon lift (core func $i "double") async)))
      (component $caller
        (import "note" (func $note (param "step" u32)))
        (import "double" (func $double async (param "x" u32) (result u32)))
        (core func $note (canon lower (func $note)))
        (core func $double (canon lower (func $double)))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "double" (func $double (param i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "run") (param i32)
            (local $doubled i32)
            (local.set $doubled (call $double (local.get 0)))
            (call $note (i32.const 3))
            (call $task-return (i32.add (local.get $doubled) (i32.const 1)))))
        (core instance $i (instantiate $m (with "" (instance
          (export "note" (func $note))
          (export "double" (func $double))
          (export "task.return" (func $task-return))))))
        (func (export "run") async (param "x" u32) (result u32)
          (canon lift (core func $i "run") async)))
      (instance $a (instantiate $callee (with "note" (func $note))))
      (instance $b (instantiate $caller
        (with "note" (func $note))
        (with "double" (func $a "double"))))
      (export "run" (func $b "run")))
    "#
);

/// A stackful caller that lowers a stackful callee of a second
/// component asynchronously.
///
/// The callee notes `1`, yields, notes `3`, and returns 7. The caller
/// notes `2` once the lower answers, waits for the call when the
/// answer says it has not returned, and returns what the callee
/// returned.
const STARTS_A_CALLEE_ASYNCHRONOUSLY: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (component $callee
        (import "note" (func $note (param "step" u32)))
        (core func $note (canon lower (func $note)))
        (core func $yield (canon thread.yield))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "yield" (func $yield (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "work")
            (call $note (i32.const 1))
            (drop (call $yield))
            (call $note (i32.const 3))
            (call $task-return (i32.const 7))))
        (core instance $i (instantiate $m (with "" (instance
          (export "note" (func $note))
          (export "yield" (func $yield))
          (export "task.return" (func $task-return))))))
        (func (export "work") async (result u32)
          (canon lift (core func $i "work") async)))
      (component $caller
        (import "note" (func $note (param "step" u32)))
        (import "work" (func $work async (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $note (canon lower (func $note)))
        (core func $work (canon lower (func $work) async (memory (core memory $libc "mem"))))
        (core func $new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core func $wait (canon waitable-set.wait (memory (core memory $libc "mem"))))
        (core func $set-drop (canon waitable-set.drop))
        (core func $subtask-drop (canon subtask.drop))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "note" (func $note (param i32)))
          (import "" "work" (func $work (param i32) (result i32)))
          (import "" "waitable-set.new" (func $new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
          (import "" "waitable-set.drop" (func $set-drop (param i32)))
          (import "" "subtask.drop" (func $subtask-drop (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "run")
            (local $status i32) (local $subtask i32) (local $set i32)
            (local.set $status (call $work (i32.const 0)))
            (call $note (i32.const 2))
            (if (i32.ne (i32.and (local.get $status) (i32.const 15)) (i32.const 2))
              (then
                (local.set $subtask (i32.shr_u (local.get $status) (i32.const 4)))
                (local.set $set (call $new))
                (call $join (local.get $subtask) (local.get $set))
                (drop (call $wait (local.get $set) (i32.const 16)))
                (call $join (local.get $subtask) (i32.const 0))
                (call $subtask-drop (local.get $subtask))
                (call $set-drop (local.get $set))))
            (call $task-return (i32.load (i32.const 0)))))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "note" (func $note))
          (export "work" (func $work))
          (export "waitable-set.new" (func $new))
          (export "waitable.join" (func $join))
          (export "waitable-set.wait" (func $wait))
          (export "waitable-set.drop" (func $set-drop))
          (export "subtask.drop" (func $subtask-drop))
          (export "task.return" (func $task-return))))))
        (func (export "run") async (result u32)
          (canon lift (core func $i "run") async)))
      (instance $a (instantiate $callee (with "note" (func $note))))
      (instance $b (instantiate $caller
        (with "note" (func $note))
        (with "work" (func $a "work"))))
      (export "run" (func $b "run")))
    "#
);

/// A synchronous export of an `async` function type that lowers the
/// host's `async` function `answer` synchronously and returns what it
/// answered.
const LOWERS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $answer (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          (call $answer (local.get 0))))
      (core instance $i (instantiate $m
        (with "" (instance (export "answer" (func $answer))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $i "run"))))
    "#
);

/// A stackful export that lowers the host's `async` function `fetch`
/// synchronously, notes `2` once it answered, and returns the answer,
/// beside a synchronous export `helper` that notes `1` and returns
/// three times its argument. The host's `fetch` calls `helper`.
const FETCHES_THROUGH_GUEST_WORK: &[u8] = component!(
    r#"
    (component
      (import "fetch" (func $fetch async (param "x" u32) (result u32)))
      (import "note" (func $note (param "step" u32)))
      (core func $fetch (canon lower (func $fetch)))
      (core func $note (canon lower (func $note)))
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "fetch" (func $fetch (param i32) (result i32)))
        (import "" "note" (func $note (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run") (param i32)
          (local $answer i32)
          (local.set $answer (call $fetch (local.get 0)))
          (call $note (i32.const 2))
          (call $task-return (local.get $answer)))
        (func (export "helper") (param i32) (result i32)
          (call $note (i32.const 1))
          (i32.mul (local.get 0) (i32.const 3))))
      (core instance $i (instantiate $m (with "" (instance
        (export "fetch" (func $fetch))
        (export "note" (func $note))
        (export "task.return" (func $task-return))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $i "run") async))
      (func (export "helper") (param "x" u32) (result u32)
        (canon lift (core func $i "helper"))))
    "#
);

/// A stackful export that makes a handle of a resource it defines,
/// whose destructor notes the handle's rep, then lowers the host's
/// `async` function `hold` synchronously, then drops the handle and
/// returns 7.
const HOLDS_A_HANDLE_ACROSS_A_BLOCK: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (import "hold" (func $hold async))
      (core func $note (canon lower (func $note)))
      (core func $hold (canon lower (func $hold)))
      (core module $dm
        (import "" "note" (func $note (param i32)))
        (func (export "dtor") (param i32) (call $note (local.get 0))))
      (core instance $d (instantiate $dm
        (with "" (instance (export "note" (func $note))))))
      (alias core export $d "dtor" (core func $dtor))
      (type $r (resource (rep i32) (dtor (core func $dtor))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "hold" (func $hold))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run")
          (local $handle i32)
          (local.set $handle (call $new (i32.const 99)))
          (call $hold)
          (call $drop (local.get $handle))
          (call $task-return (i32.const 7))))
      (core instance $i (instantiate $m (with "" (instance
        (export "hold" (func $hold))
        (export "new" (func $new))
        (export "drop" (func $drop))
        (export "task.return" (func $task-return))))))
      (func (export "run") async (result u32)
        (canon lift (core func $i "run") async)))
    "#
);

/// A stackful export that waits on a waitable set of its own, which
/// nothing ever fills.
const WAITS_ON_AN_EMPTY_SET: &[u8] = component!(
    r#"
    (component
      (core module $libc (memory (export "mem") 1))
      (core instance $libc (instantiate $libc))
      (core func $new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "mem"))))
      (core module $m
        (import "" "waitable-set.new" (func $new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (func (export "run")
          (drop (call $wait (call $new) (i32.const 0)))))
      (core instance $i (instantiate $m (with "" (instance
        (export "waitable-set.new" (func $new))
        (export "waitable-set.wait" (func $wait))))))
      (func (export "run") async (result u32)
        (canon lift (core func $i "run") async)))
    "#
);

/// A stackful export whose implicit thread starts an explicit thread
/// that blocks, and then traps while that thread still waits, which
/// ends the task with the thread in the middle of a blocking
/// built-in. A task ends with its last thread when nothing fails, so
/// a trap is what ends a task whose thread still waits.
///
/// Each of `wait`, `read`, and `hold` yields to a new thread with
/// `thread.yield-then-resume` and traps once it resumes. The new
/// thread blocks for ever: `wait` on a waitable set nothing fills,
/// `read` in a synchronous read of a stream nothing writes, and
/// `hold` in a synchronous lower of the host `async` function
/// `hold`. The synchronous exports `drop-set` and `join-end` then
/// drop that set and join that end to a new set.
const ENDS_WITH_A_THREAD_BLOCKED: &[u8] = component!(
    r#"
    (component
      (import "hold" (func $hold async))
      (core module $libc
        (memory (export "memory") 1)
        (table (export "__indirect_function_table") 3 funcref))
      (core instance $libc (instantiate $libc))
      (type $s (stream u8))
      (core func $hold (canon lower (func $hold)))
      (core func $task-return (canon task.return (result u32)))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $set-new (canon waitable-set.new))
      (core func $set-drop (canon waitable-set.drop))
      (core func $join (canon waitable.join))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $stream-new (canon stream.new $s))
      (core func $read (canon stream.read $s (memory (core memory $libc "memory"))))
      (core module $m
        (import "" "hold" (func $hold))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.drop" (func $set-drop (param i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "stream.new" (func $stream-new (result i64)))
        (import "" "stream.read" (func $read (param i32 i32 i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 3 funcref))
        (global $set (mut i32) (i32.const 0))
        (global $end (mut i32) (i32.const 0))
        (func $wait-on-set (param i32)
          (drop (call $wait (global.get $set) (i32.const 0))))
        (func $read-end (param i32)
          (drop (call $read (global.get $end) (i32.const 16) (i32.const 1))))
        (func $hold-call (param i32)
          (call $hold))
        (elem (table 0) (i32.const 0) func $wait-on-set $read-end $hold-call)
        (func $run (param $entry i32)
          (drop (call $yield-then-resume
            (call $new-indirect (local.get $entry) (i32.const 0))))
          unreachable)
        (func (export "wait")
          (global.set $set (call $set-new))
          (call $run (i32.const 0)))
        (func (export "read")
          (global.set $end (i32.wrap_i64 (call $stream-new)))
          (call $run (i32.const 1)))
        (func (export "hold")
          (call $run (i32.const 2)))
        (func (export "drop-set") (result i32)
          (call $set-drop (global.get $set))
          (i32.const 0))
        (func (export "join-end") (result i32)
          (call $join (global.get $end) (call $set-new))
          (i32.const 0)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "hold" (func $hold))
          (export "task.return" (func $task-return))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.drop" (func $set-drop))
          (export "waitable.join" (func $join))
          (export "waitable-set.wait" (func $wait))
          (export "stream.new" (func $stream-new))
          (export "stream.read" (func $read))))
        (with "libc" (instance $libc))))
      (func (export "wait") async (result u32)
        (canon lift (core func $i "wait") async))
      (func (export "read") async (result u32)
        (canon lift (core func $i "read") async))
      (func (export "hold") async (result u32)
        (canon lift (core func $i "hold") async))
      (func (export "drop-set") (result u32)
        (canon lift (core func $i "drop-set")))
      (func (export "join-end") (result u32)
        (canon lift (core func $i "join-end"))))
    "#
);

/// A component whose stackful exports switch between threads with the
/// thread built-ins that suspend or switch.
///
/// The component keeps a log in a core global, one digit per step, and
/// each export returns the log. The export's own thread writes 1 as it
/// begins, 3 once it goes on after its first built-in, and 5 as it
/// ends. The thread it starts writes 2 as it begins, and 4 once it goes
/// on after a built-in of its own.
///
/// - `resume-suspended` yields to a new thread that suspends itself,
///   then switches to that suspended thread with
///   `thread.suspend-then-resume`. The thread goes on, makes the
///   export's thread ready, and returns.
/// - `promote-ready` yields to a new thread that yields in turn, then
///   switches to that ready thread with `thread.suspend-then-promote`.
///   The thread goes on, makes the export's thread ready, and returns.
/// - `switch-back` switches to a new thread with
///   `thread.suspend-then-resume`, and that thread switches straight
///   back to the export's thread, which returns while the new thread
///   is still suspended.
const SWITCHES: &[u8] = component!(
    r#"
    (component
      (core module $libc (table (export "__indirect_function_table") 3 funcref))
      (core instance $libc (instantiate $libc))
      (core func $task-return (canon task.return (result u32)))
      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))
      (core func $yield (canon thread.yield))
      (core func $suspend (canon thread.suspend))
      (core func $suspend-then-resume (canon thread.suspend-then-resume))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $suspend-then-promote (canon thread.suspend-then-promote))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "" "thread.yield" (func $yield (result i32)))
        (import "" "thread.suspend" (func $suspend (result i32)))
        (import "" "thread.suspend-then-resume" (func $suspend-then-resume (param i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "thread.suspend-then-promote" (func $suspend-then-promote (param i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 3 funcref))
        (global $main (mut i32) (i32.const 0))
        (global $log (mut i32) (i32.const 0))
        (func $log (param i32)
          (global.set $log
            (i32.add (i32.mul (global.get $log) (i32.const 10)) (local.get 0))))
        (func $suspends (param i32)
          (call $log (i32.const 2))
          (drop (call $suspend))
          (call $log (i32.const 4))
          (call $resume-later (global.get $main)))
        (func $yields (param i32)
          (call $log (i32.const 2))
          (drop (call $yield))
          (call $log (i32.const 4))
          (call $resume-later (global.get $main)))
        (func $switches-back (param i32)
          (call $log (i32.const 2))
          (drop (call $suspend-then-resume (global.get $main))))
        (elem (table 0) (i32.const 0) func $suspends $yields $switches-back)
        (func $begin
          (global.set $main (call $thread-index))
          (global.set $log (i32.const 0))
          (call $log (i32.const 1)))
        (func $end
          (call $log (i32.const 5))
          (call $task-return (global.get $log)))
        (func (export "resume-suspended")
          (local $thread i32)
          (call $begin)
          (local.set $thread (call $new-indirect (i32.const 0) (i32.const 0)))
          (drop (call $yield-then-resume (local.get $thread)))
          (call $log (i32.const 3))
          (drop (call $suspend-then-resume (local.get $thread)))
          (call $end))
        (func (export "promote-ready")
          (local $thread i32)
          (call $begin)
          (local.set $thread (call $new-indirect (i32.const 1) (i32.const 0)))
          (drop (call $yield-then-resume (local.get $thread)))
          (call $log (i32.const 3))
          (drop (call $suspend-then-promote (local.get $thread)))
          (call $end))
        (func (export "switch-back")
          (call $begin)
          (drop (call $suspend-then-resume (call $new-indirect (i32.const 2) (i32.const 0))))
          (call $log (i32.const 3))
          (call $task-return (global.get $log))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))
          (export "thread.yield" (func $yield))
          (export "thread.suspend" (func $suspend))
          (export "thread.suspend-then-resume" (func $suspend-then-resume))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "thread.suspend-then-promote" (func $suspend-then-promote))))
        (with "libc" (instance $libc))))
      (func (export "resume-suspended") async (result u32)
        (canon lift (core func $i "resume-suspended") async))
      (func (export "promote-ready") async (result u32)
        (canon lift (core func $i "promote-ready") async))
      (func (export "switch-back") async (result u32)
        (canon lift (core func $i "switch-back") async)))
    "#
);

/// What the guest noted, in order.
type Notes = Arc<Mutex<Vec<u32>>>;

/// An engine that accepts the stackful lift, the thread built-ins, and
/// the synchronous forms of the stream and future built-ins,
/// with the suspend provider allowed or turned off.
fn engine(provider: bool) -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_threading(true);
    config.wasm_component_model_more_async_builtins(true);
    config.suspend_provider(provider);
    Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine")
}

/// An engine with the stackful async and threading features, over a
/// backend that declares host suspension, so it selects the
/// host-suspension provider on every target: Wasmi natively, and the
/// browser's engine in the browser.
fn suspending_engine() -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_threading(true);
    config.wasm_component_model_more_async_builtins(true);
    Engine::with_backend(crate::runtime_layer::test_suspending_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine")
}

/// Whether `engine` runs guest threads through a provider: the
/// stack-switching provider natively on x86_64 Linux, and the
/// host-suspension provider in a browser that ships JavaScript Promise
/// Integration.
fn has_provider(engine: &Engine) -> bool {
    engine.suspend_provider() != SuspendProviderKind::None
}

/// A linker whose `note` records each step in `notes`.
fn noting<T: 'static>(engine: &Engine, notes: &Notes) -> Linker<T> {
    let mut linker: Linker<T> = Linker::new(engine);
    let recorded = notes.clone();
    linker
        .root()
        .func_wrap(
            "note",
            move |_: HostCall<'_, T>, (step,): (u32,)| -> Result<(), Error> {
                recorded.lock().expect("notes").push(step);
                Ok(())
            },
        )
        .expect("the registration");
    linker
}

/// Instantiate `bytes` with `linker` into a fresh store of `engine`.
async fn instantiate(engine: &Engine, linker: &Linker<()>, bytes: &[u8]) -> (Store<()>, Instance) {
    let component = Component::new(engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

/// How many nested turns the store's suspend seam has run.
fn nested_turns(store: &mut Store<()>) -> u64 {
    store
        .internal()
        .context()
        .internal()
        .scheduler()
        .nested_turns()
}

/// How many threads are suspended in the store's provider.
fn parked_threads(store: &mut Store<()>) -> usize {
    store
        .internal()
        .context()
        .internal()
        .scheduler()
        .parked_threads()
}

/// Every message in an error's source chain, on one line.
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
async fn it_suspends_a_stackful_export_in_a_second_component_and_resumes_it() {
    // The caller's thread starts through the provider, and its
    // synchronous lower of the callee is a nested start: the callee's
    // thread runs on a stack of its own above the start intrinsic,
    // behind a fused adapter. The callee's yield suspends it there,
    // the start returns to the intrinsic, and the caller suspends too,
    // waiting on the call. The driver then gives the host executor its
    // turn, and the next poll resumes the callee, whose return makes
    // the caller ready. Without a provider the callee's yield runs a
    // nested turn and the whole call runs in one poll.
    let engine = engine(true);
    let notes: Notes = Arc::default();
    let linker = noting(&engine, &notes);
    let (mut store, instance) = instantiate(&engine, &linker, CALLS_A_SECOND_COMPONENT).await;
    let run = func(&instance, "run");
    let provider = has_provider(&engine);

    let result = {
        let mut call = Box::pin(run.call(&mut store, &[Val::U32(20)]));
        let first = poll_once(&mut call);
        if provider {
            assert!(
                first.is_pending(),
                "the callee suspended in its yield, and the driver returned to the executor"
            );
            assert_eq!(
                notes.lock().expect("notes").clone(),
                vec![1],
                "the callee ran up to its yield, and nothing after it has run"
            );
            call.await
        } else {
            match first {
                Poll::Ready(result) => result,
                Poll::Pending => call.await,
            }
        }
    }
    .expect("the call returns once the callee resumed and returned");

    assert_eq!(result.as_ref(), [Val::U32(41)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![1, 2, 3],
        "the callee resumed after its yield, then the caller's lower returned"
    );
    if provider {
        assert_eq!(
            nested_turns(&mut store),
            0,
            "both threads suspended in the provider, so no nested turn ran"
        );
        assert_eq!(parked_threads(&mut store), 0, "no thread is left suspended");
    } else {
        assert!(nested_turns(&mut store) > 0, "the yield took a nested turn");
    }
}

#[wcmp_macros::test]
async fn it_resumes_a_nested_start_after_the_trampoline_that_started_it_returned() {
    // The caller's asynchronous lower starts the callee from inside
    // the start intrinsic, a host trampoline, as a nested start. The
    // callee yields and suspends, the start returns to the
    // trampoline, and the trampoline answers `STARTED` to the caller,
    // which notes `2` and waits on the call. The callee resumes only
    // after that, so it notes `3` after its starter noted `2`. Without
    // a provider the callee runs on the real stack above the
    // trampoline, its yield takes a nested turn, and it returns
    // before the trampoline does.
    let engine = engine(true);
    let notes: Notes = Arc::default();
    let linker = noting(&engine, &notes);
    let (mut store, instance) = instantiate(&engine, &linker, STARTS_A_CALLEE_ASYNCHRONOUSLY).await;

    let result = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect("the caller returns what the callee returned");

    assert_eq!(result.as_ref(), [Val::U32(7)]);
    let expected = if has_provider(&engine) {
        vec![1, 2, 3]
    } else {
        vec![1, 3, 2]
    };
    assert_eq!(notes.lock().expect("notes").clone(), expected);
    if has_provider(&engine) {
        assert_eq!(nested_turns(&mut store), 0);
        assert_eq!(parked_threads(&mut store), 0);
    }
}

/// The state of a host future the test releases by hand: whether it
/// is released, how often it was polled, and the waker of its last
/// pending poll.
#[derive(Default)]
struct Gate {
    released: bool,
    polls: u32,
    waker: Option<Waker>,
}

/// A host future that is pending until its gate is released, and
/// answers `value` then.
struct Gated {
    gate: Arc<Mutex<Gate>>,
    value: u32,
}

impl Future for Gated {
    type Output = Result<u32, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut gate = self.gate.lock().expect("gate");
        gate.polls += 1;
        if gate.released {
            return Poll::Ready(Ok(self.value));
        }
        gate.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

/// Instantiate [`LOWERS_A_HOST_ASYNC_FUNCTION`] into a store of an
/// engine with the provider allowed or not, with `answer` pending on
/// `gate` and answering twice its argument.
async fn gated_answer(provider: bool, gate: &Arc<Mutex<Gate>>) -> (Engine, Store<()>, Func) {
    let engine = engine(provider);
    let mut linker: Linker<()> = Linker::new(&engine);
    let gate = gate.clone();
    linker
        .root()
        .func_wrap_concurrent("answer", move |_accessor: &Accessor<()>, (x,): (u32,)| {
            Gated {
                gate: gate.clone(),
                value: x * 2,
            }
        })
        .expect("the registration");
    let (store, instance) = instantiate(&engine, &linker, LOWERS_A_HOST_ASYNC_FUNCTION).await;
    let run = func(&instance, "run");
    (engine, store, run)
}

#[wcmp_macros::test]
async fn it_returns_a_host_async_result_to_a_synchronous_export_under_the_provider() {
    // The export is lifted synchronously, and its function type is
    // `async`, so its task is allowed to block. Its lower of `answer`
    // finds the future pending, parks it among the store's host
    // tasks, and suspends the thread. The turn polls the future once
    // more, and it is still pending, so the driver returns to the
    // executor. The host releases the future, and the next poll of
    // the driver completes it, resumes the thread, and the export
    // returns the host function's result.
    let gate: Arc<Mutex<Gate>> = Arc::default();
    let (engine, mut store, run) = gated_answer(true, &gate).await;
    if !has_provider(&engine) {
        // No provider on this target: the lanes that have one prove
        // the rest, and the test below proves this target's answer.
        return;
    }

    let result = {
        let mut call = Box::pin(run.call(&mut store, &[Val::U32(21)]));
        assert!(
            poll_once(&mut call).is_pending(),
            "the thread waits on the host future, suspended"
        );
        assert_eq!(
            gate.lock().expect("gate").polls,
            2,
            "the future was pending for two polls: the lower's, and the turn's"
        );
        let waker = {
            let mut gate = gate.lock().expect("gate");
            gate.released = true;
            gate.waker
                .take()
                .expect("the pending future kept its waker")
        };
        waker.wake();
        call.await
    }
    .expect("the export returns the host function's result");

    assert_eq!(result.as_ref(), [Val::U32(42)]);
    assert_eq!(nested_turns(&mut store), 0);
}

#[wcmp_macros::test]
async fn it_fails_a_host_async_lower_of_a_synchronous_export_with_the_stack_switch_cause_without_a_provider()
 {
    // The same call with the provider turned off. The lower waits in
    // a nested turn, which polls the future and finds nothing else to
    // run. The future is released only from outside the call, so the
    // wait cannot end, and the call fails with the stack-switch
    // cause: a stack switch would have served it.
    let gate: Arc<Mutex<Gate>> = Arc::default();
    let (engine, mut store, run) = gated_answer(false, &gate).await;
    assert_eq!(engine.suspend_provider(), SuspendProviderKind::None);

    let err = run
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect_err("a future released only from outside the call cannot be waited for");

    assert!(
        chain(&err).contains("blocking here requires a stack switch"),
        "expected the stack-switch cause, got {err:?}"
    );
    assert!(
        nested_turns(&mut store) > 0,
        "the lower waited in nested turns"
    );
}

#[wcmp_macros::test]
async fn it_resumes_a_thread_blocked_on_a_host_task_that_runs_guest_work_without_a_nested_turn() {
    // The stackful export lowers `fetch` synchronously. `fetch` runs
    // guest work through its accessor: it calls the component's
    // `helper`, which a turn runs as an item of its own, and answers
    // with what `helper` returned. The export's thread waits on the
    // host task, suspended in the provider, while turns run `helper`
    // and poll the body. It resumes once a turn polls the body to its
    // end, which is after `helper` noted `1`. No readiness condition
    // runs guest code, so no nested turn runs, and the body finds the
    // provider in the store while the thread is suspended.
    let engine = engine(true);
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    let helper: Arc<Mutex<Option<Arc<Func>>>> = Arc::default();
    let seen: Arc<Mutex<Option<(bool, usize)>>> = Arc::default();
    {
        let helper = helper.clone();
        let seen = seen.clone();
        linker
            .root()
            .func_wrap_concurrent("fetch", move |accessor: &Accessor<()>, (x,): (u32,)| {
                let accessor = accessor.clone();
                let helper = helper.lock().expect("helper").clone().expect("helper set");
                let seen = seen.clone();
                async move {
                    let tripled = helper.call_concurrent(&accessor, &[Val::U32(x)]).await?;
                    let observed = accessor.with(|store| {
                        let provider = store.internal().provider().is_some();
                        let parked = store.internal().scheduler().parked_threads();
                        (provider, parked)
                    })?;
                    *seen.lock().expect("seen") = Some(observed);
                    match tripled.first() {
                        Some(&Val::U32(value)) => Ok(value),
                        other => panic!("`helper` returned {other:?}"),
                    }
                }
            })
            .expect("the registration");
    }
    let (mut store, instance) = instantiate(&engine, &linker, FETCHES_THROUGH_GUEST_WORK).await;
    *helper.lock().expect("helper") = Some(Arc::new(func(&instance, "helper")));
    let run = func(&instance, "run");

    let outcome = run.call(&mut store, &[Val::U32(5)]).await;

    if !has_provider(&engine) {
        // Without a provider the export's lower waits in nested turns
        // on the real stack. Those turns run `helper` and poll the
        // body, so the call returns all the same, through nested
        // turns.
        let result = outcome.expect("the nested turns run the host task's guest work");
        assert_eq!(result.as_ref(), [Val::U32(15)]);
        assert!(nested_turns(&mut store) > 0);
        return;
    }
    let result = outcome.expect("the thread resumes once the host task completes");
    assert_eq!(result.as_ref(), [Val::U32(15)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![1, 2],
        "the host task's guest work ran before the thread resumed"
    );
    assert_eq!(
        *seen.lock().expect("seen"),
        Some((true, 1)),
        "the body found the provider in the store, with the export's thread suspended in it"
    );
    assert_eq!(
        nested_turns(&mut store),
        0,
        "the suspension ran no nested turn"
    );
}

/// A host future that is pending on its first poll, waking its waker,
/// and ready afterwards.
struct PendingOnce {
    polled: bool,
}

impl Future for PendingOnce {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.polled {
            return Poll::Ready(Ok(()));
        }
        self.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

#[wcmp_macros::test]
async fn it_drops_a_store_with_a_suspended_thread_and_runs_the_export_again_in_a_later_store() {
    // The export makes a handle of its own resource, and then waits
    // on `hold`. In the first store `hold` never resolves, so the
    // export's thread stays suspended, holding the handle, until the
    // test drops the store. Nothing resumes the thread and no
    // destructor runs. A second store of the same engine runs the
    // same export to its end, where `hold` resolves on its second
    // poll and the export drops its handle, which runs the
    // destructor.
    let engine = engine(true);
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    let resolves = Arc::new(AtomicBool::new(false));
    {
        let resolves = resolves.clone();
        linker
            .root()
            .func_wrap_concurrent("hold", move |_accessor: &Accessor<()>, (): ()| {
                let resolves = resolves.load(Ordering::SeqCst);
                async move {
                    if resolves {
                        PendingOnce { polled: false }.await
                    } else {
                        core::future::pending::<Result<(), Error>>().await
                    }
                }
            })
            .expect("the registration");
    }
    let component = Component::new(&engine, HOLDS_A_HANDLE_ACROSS_A_BLOCK)
        .await
        .expect("component parses");

    {
        let mut store: Store<()> = Store::new(&engine, ()).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = func(&instance, "run");
        let first = {
            let mut call = Box::pin(run.call(&mut store, &[]));
            poll_once(&mut call)
        };
        if has_provider(&engine) {
            assert!(
                first.is_pending(),
                "the export's thread waits on `hold`, suspended"
            );
            assert_eq!(parked_threads(&mut store), 1);
        } else {
            // Without a provider the wait fails at once, because no
            // nested turn can resolve `hold`, and the export's thread
            // unwinds without dropping its handle.
            let Poll::Ready(Err(err)) = first else {
                panic!("the call fails at once without a provider, got {first:?}");
            };
            assert!(chain(&err).contains("blocking here requires a stack switch"));
        }
        drop(store);
    }
    assert!(
        notes.lock().expect("notes").is_empty(),
        "dropping the store ran no destructor"
    );

    resolves.store(true, Ordering::SeqCst);
    let mut store: Store<()> = Store::new(&engine, ()).expect("a later store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let result = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect("the export runs to its end in the later store");
    assert_eq!(result.as_ref(), [Val::U32(7)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![99],
        "the export dropped its handle, which ran the destructor"
    );
}

#[wcmp_macros::test]
async fn it_reuses_the_workers_of_finished_threads() {
    // Each call runs two threads, the caller's and the callee's, and
    // each suspends once. A worker whose thread finished waits in the
    // switch module's pool, and the next thread runs on it, so the
    // store never holds more workers than it had threads alive at
    // once, however many calls it runs. The host-suspension provider has no
    // workers, since the browser keeps each stack, and runs the same
    // calls.
    let engine = engine(true);
    if !has_provider(&engine) {
        return;
    }
    let notes: Notes = Arc::default();
    let linker = noting(&engine, &notes);
    let (mut store, instance) = instantiate(&engine, &linker, CALLS_A_SECOND_COMPONENT).await;
    let run = func(&instance, "run");

    for x in 0..50 {
        let result = run
            .call(&mut store, &[Val::U32(x)])
            .await
            .expect("the call returns");
        assert_eq!(result.as_ref(), [Val::U32(2 * x + 1)]);
    }

    let mut context = store.internal().context();
    let Some(StoreProvider::StackSwitching(provider)) = context.internal().provider() else {
        return;
    };
    assert_eq!(
        provider.workers(&mut context).expect("the worker count"),
        2,
        "fifty calls of two threads each ran on two workers"
    );
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

#[wcmp_macros::test]
async fn it_fails_a_driver_whose_store_goes_idle_while_a_thread_is_suspended_with_the_deadlock_cause()
 {
    // The export's thread waits on a set nothing fills. Under a
    // provider it suspends, the turn finds nothing to run and no host
    // task, and the driver fails with the deadlock cause: nothing left
    // in the store can resume the thread. The thread fails as the same
    // wait fails with no provider, so its task leaves the store.
    let engine = engine(true);
    let linker: Linker<()> = Linker::new(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, WAITS_ON_AN_EMPTY_SET).await;

    let err = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect_err("nothing can fill the set");

    assert!(
        chain(&err).contains("deadlock detected: event loop cannot make further progress"),
        "expected the deadlock cause, got {err:?}"
    );
    assert_eq!(task_count(&store), 0, "the export's task left the store");
    assert_eq!(parked_threads(&mut store), 0, "no thread is left suspended");
}

#[wcmp_macros::test]
async fn it_leaves_run_concurrent_pending_while_a_thread_is_suspended_in_an_idle_store() {
    // The same wait under the store's concurrent entry. Its closure can
    // still unblock the thread with another call, so an idle store
    // leaves the entry pending rather than failing it.
    let engine = engine(true);
    if !has_provider(&engine) {
        return;
    }
    let linker: Linker<()> = Linker::new(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, WAITS_ON_AN_EMPTY_SET).await;
    let run = func(&instance, "run");

    {
        let mut entry = Box::pin(
            store.run_concurrent(async |accessor| run.call_concurrent(accessor, &[]).await),
        );
        assert!(
            poll_once(&mut entry).is_pending(),
            "the concurrent entry waits while the thread is suspended"
        );
        assert!(poll_once(&mut entry).is_pending(), "and goes on waiting");
    }
    assert_eq!(
        parked_threads(&mut store),
        1,
        "the thread is still suspended in the provider"
    );
}

/// A linker whose host `async` function `hold` never resolves.
fn holding(engine: &Engine) -> Linker<()> {
    let mut linker: Linker<()> = Linker::new(engine);
    linker
        .root()
        .func_wrap_concurrent("hold", |_accessor: &Accessor<()>, (): ()| async {
            core::future::pending::<Result<(), Error>>().await
        })
        .expect("the registration");
    linker
}

/// How many host tasks the store holds.
fn host_tasks(store: &mut Store<()>) -> usize {
    store
        .internal()
        .context()
        .internal()
        .scheduler()
        .host_task_count()
}

/// Call `name` of [`ENDS_WITH_A_THREAD_BLOCKED`], whose task ends
/// while a thread it started is blocked, and check how the call went.
///
/// Under a provider the export traps with `unreachable`: the thread it
/// yielded to suspended in its blocking built-in, the export's thread
/// resumed after it and trapped, and the blocked thread left the store
/// with the task the trap ended. Without a provider the started thread
/// runs on the real stack above the export's thread, and its block
/// fails with the stack-switch cause, since the export's thread below
/// it would go on under a stack switch.
async fn ends_with_a_thread_blocked(
    engine: &Engine,
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
) {
    let outcome = func(instance, name).call(&mut *store, &[]).await;
    if has_provider(engine) {
        let err = outcome.expect_err("the export traps once it resumes");
        assert!(
            chain(&err).contains("unreachable"),
            "expected the export's trap, got {err:?}"
        );
        assert_eq!(
            parked_threads(store),
            0,
            "the blocked thread left the store with its task"
        );
    } else {
        let err = outcome.expect_err("the started thread cannot block above the export's thread");
        assert!(
            chain(&err).contains("blocking here requires a stack switch"),
            "expected the stack-switch cause, got {err:?}"
        );
    }
    assert_eq!(task_count(store), 0, "the export's task left the store");
}

#[wcmp_macros::test]
async fn it_gives_back_the_wait_of_a_thread_whose_task_ended_while_it_waited_on_a_set() {
    // The thread's `waitable-set.wait` raised the set's tally of
    // waiters. Its task ends while it waits, so the built-in's finish
    // part runs then, and the tally falls: a later drop of the set
    // finds no waiter.
    let engine = engine(true);
    let linker = holding(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, ENDS_WITH_A_THREAD_BLOCKED).await;

    ends_with_a_thread_blocked(&engine, &mut store, &instance, "wait").await;

    // The trap poisoned the store, so the tally is read from the
    // store's records rather than through a drop of the set.
    assert_eq!(
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("handle tables")
            .tasks
            .set_waiters(),
        0,
        "the set has no waiter left"
    );
}

#[wcmp_macros::test]
async fn it_gives_back_the_synchronous_wait_of_a_thread_whose_task_ended_while_it_read() {
    // The thread's synchronous `stream.read` marked the readable end
    // as waited on synchronously, which keeps it out of every set.
    // Its task ends while it waits, and the mark comes off with the
    // built-in's finish part: a later join of the end succeeds.
    let engine = engine(true);
    let linker = holding(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, ENDS_WITH_A_THREAD_BLOCKED).await;

    ends_with_a_thread_blocked(&engine, &mut store, &instance, "read").await;

    // The trap poisoned the store, so the mark is read from the
    // store's records rather than through a join of the end.
    assert_eq!(
        store
            .internal_ref()
            .tables()
            .lock()
            .expect("handle tables")
            .tasks
            .synchronous_end_waiters(),
        0,
        "no thread waits on the end synchronously"
    );
}

#[wcmp_macros::test]
async fn it_withdraws_the_host_task_of_a_thread_whose_task_ended_inside_a_synchronous_lower() {
    // The thread's synchronous lower of `hold` parked the host
    // function's future in the store as a host task. Its task ends
    // while it waits, and the lower's finish part takes the task out
    // of the store, so no host future is left pending for a call that
    // no longer exists.
    let engine = engine(true);
    let linker = holding(&engine);
    let (mut store, instance) = instantiate(&engine, &linker, ENDS_WITH_A_THREAD_BLOCKED).await;

    ends_with_a_thread_blocked(&engine, &mut store, &instance, "hold").await;

    assert_eq!(host_tasks(&mut store), 0, "no host task is left pending");
}

/// Call `name` of [`SWITCHES`] in a fresh store of `engine`, and
/// answer its outcome with the number of nested turns the store ran
/// and the number of threads left suspended in the provider.
async fn switch(engine: &Engine, name: &str) -> (Result<u32, Error>, u64, usize) {
    let linker: Linker<()> = Linker::new(engine);
    let (mut store, instance) = instantiate(engine, &linker, SWITCHES).await;
    let outcome = func(&instance, name)
        .call(&mut store, &[])
        .await
        .map(|values| match values.first() {
            Some(Val::U32(log)) => *log,
            other => panic!("`{name}` answered {other:?}"),
        });
    let parked = parked_threads(&mut store);
    (outcome, nested_turns(&mut store), parked)
}

#[wcmp_macros::test]
async fn it_resumes_a_thread_suspended_in_the_provider_that_a_switch_names() {
    // The new thread suspends itself in the provider, and the export's
    // `thread.suspend-then-resume` names it. The turn that resumed the
    // export's thread runs the named thread next, on its own stack, and
    // that thread makes the export's thread ready before it returns.
    // Without a provider the new thread runs on the real stack above
    // the export's thread, which is where its suspension waits, and
    // nothing above it can resume it: the block fails with the
    // stack-switch cause.
    let engine = engine(true);
    let (outcome, turns, parked) = switch(&engine, "resume-suspended").await;
    assert_eq!(parked, 0, "no thread is left suspended");
    if has_provider(&engine) {
        assert_eq!(outcome.expect("the export returns"), 12345);
        assert_eq!(turns, 0, "every suspension went through the provider");
    } else {
        let err = outcome.expect_err("the suspended thread lies above its resumer");
        assert!(
            chain(&err).contains("blocking here requires a stack switch"),
            "expected the stack-switch cause, got {err:?}"
        );
    }
}

#[wcmp_macros::test]
async fn it_switches_to_a_ready_thread_suspended_in_the_provider_that_a_promote_names() {
    // The new thread yields and waits, ready, in the provider. The
    // export's `thread.suspend-then-promote` finds it ready, and the
    // turn that resumed the export's thread resumes it next. Without a
    // provider the new thread's yield returns on the real stack above
    // the export's thread, which is not suspended but yielding, so its
    // resume of that thread fails with Wasmtime's message.
    let engine = engine(true);
    let (outcome, turns, parked) = switch(&engine, "promote-ready").await;
    assert_eq!(parked, 0, "no thread is left suspended");
    if has_provider(&engine) {
        assert_eq!(outcome.expect("the export returns"), 12345);
        assert_eq!(turns, 0, "every suspension went through the provider");
    } else {
        let err = outcome.expect_err("the yielding thread is not suspended");
        assert!(
            chain(&err).contains("cannot resume thread which is not suspended"),
            "expected the not-suspended trap, got {err:?}"
        );
    }
}

#[wcmp_macros::test]
async fn it_runs_a_chain_of_switches_from_the_frame_that_resumed_the_first() {
    // The export's thread switches to a new thread, which switches
    // straight back. Both switches run from the frame that started the
    // export's thread, one after the other, and the export returns
    // while the new thread stays suspended, and the task with it, since
    // a task lives until its last thread ends.
    // Without a provider the new thread runs above the export's thread
    // on the real stack, and its switch back fails with the
    // stack-switch cause.
    let engine = engine(true);
    let (outcome, turns, parked) = switch(&engine, "switch-back").await;
    if has_provider(&engine) {
        assert_eq!(outcome.expect("the export returns"), 123);
        assert_eq!(turns, 0, "every suspension went through the provider");
        assert_eq!(
            parked, 1,
            "the new thread is still suspended after the export returned"
        );
    } else {
        let err = outcome.expect_err("the export's thread lies below the new thread");
        assert!(
            chain(&err).contains("blocking here requires a stack switch"),
            "expected the stack-switch cause, got {err:?}"
        );
    }
}

/// Two component instances: one whose synchronous export resumes a
/// suspended thread of its own instance from inside its own call, and
/// one that keeps the rest of the store busy.
///
/// In the first, `setup` starts a worker thread, which suspends
/// itself, then notes `1`, returns, and suspends its own thread for
/// good. `wake` is sync-typed, so its task must not block. It notes
/// `2` and switches to the worker with `thread.yield-then-resume`,
/// which resumes the worker from inside the call: the worker notes
/// `3` and ends. `wake` goes on once it has, notes `4`, and returns 7,
/// or traps when the worker has not run.
///
/// In the second, `spin` lowers the host's `tick` asynchronously and
/// yields for ever: its callback notes `200` each time it runs, and
/// `tick`'s host task, which notes `100` on each poll, wakes itself on
/// each poll.
const WAKES_ITS_OWN_WORKER: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (import "tick" (func $tick async))
      (component $own
        (import "note" (func $note (param "step" u32)))
        (core module $libc (table (export "__indirect_function_table") 1 funcref))
        (core instance $libc (instantiate $libc))
        (core func $note (canon lower (func $note)))
        (core func $task-return (canon task.return (result u32)))
        (core type $start-ty (func (param i32)))
        (alias core export $libc "__indirect_function_table" (core table $table))
        (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
        (core func $resume-later (canon thread.resume-later))
        (core func $yield (canon thread.yield))
        (core func $suspend (canon thread.suspend))
        (core func $yield-then-resume (canon thread.yield-then-resume))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
          (import "" "thread.resume-later" (func $resume-later (param i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (import "" "thread.suspend" (func $suspend (result i32)))
          (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
          (import "libc" "__indirect_function_table" (table 1 funcref))
          (global $worker (mut i32) (i32.const 0))
          (global $done (mut i32) (i32.const 0))
          (func $work (param i32)
            (drop (call $suspend))
            (call $note (i32.const 3))
            (global.set $done (i32.const 1)))
          (elem (table 0) (i32.const 0) func $work)
          (func (export "setup")
            (global.set $worker (call $new-indirect (i32.const 0) (i32.const 0)))
            (call $resume-later (global.get $worker))
            (drop (call $yield))
            (call $note (i32.const 1))
            (call $task-return (i32.const 0))
            (drop (call $suspend)))
          (func (export "wake") (result i32)
            (call $note (i32.const 2))
            (drop (call $yield-then-resume (global.get $worker)))
            (if (i32.eqz (global.get $done)) (then unreachable))
            (call $note (i32.const 4))
            (i32.const 7)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "note" (func $note))
            (export "task.return" (func $task-return))
            (export "thread.new-indirect" (func $new-indirect))
            (export "thread.resume-later" (func $resume-later))
            (export "thread.yield" (func $yield))
            (export "thread.suspend" (func $suspend))
            (export "thread.yield-then-resume" (func $yield-then-resume))))
          (with "libc" (instance $libc))))
        (func (export "setup") async (result u32)
          (canon lift (core func $i "setup") async))
        (func (export "wake") (result u32)
          (canon lift (core func $i "wake"))))
      (component $other
        (import "note" (func $note (param "step" u32)))
        (import "tick" (func $tick async))
        (core func $note (canon lower (func $note)))
        (core func $tick (canon lower (func $tick) async))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "tick" (func $tick (result i32)))
          (func (export "spin") (result i32)
            (drop (call $tick))
            (call $note (i32.const 200))
            (i32.const 1))
          (func (export "spin-again") (param i32 i32 i32) (result i32)
            (call $note (i32.const 200))
            (i32.const 1)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "note" (func $note))
            (export "tick" (func $tick))))))
        (func (export "spin") async
          (canon lift (core func $i "spin") async (callback (core func $i "spin-again")))))
      (instance $a (instantiate $own (with "note" (func $note))))
      (instance $b (instantiate $other (with "note" (func $note)) (with "tick" (func $tick))))
      (export "setup" (func $a "setup"))
      (export "wake" (func $a "wake"))
      (export "spin" (func $b "spin")))
    "#
);

/// A host future that notes `100` in `notes` on each poll, wakes its
/// waker, and never completes: a host task the store polls in every
/// turn that polls host tasks.
struct Ticks {
    notes: Notes,
}

impl Future for Ticks {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.notes.lock().expect("notes").push(100);
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

#[wcmp_macros::test]
async fn it_resumes_a_thread_of_its_own_instance_from_a_synchronous_export_and_nothing_else() {
    // `wake`'s task must not block, so its thread cannot suspend its
    // stack, and its switch to the worker runs the worker from inside
    // the built-in, as the reference resumes a thread from inside the
    // trampoline and goes on in it once the thread stops. Under the
    // stack-switching provider the built-in resumes the worker in
    // place. The host-suspension provider cannot resume a stack from inside a
    // call, so `wake`'s own thread suspends as well, and the scheduler
    // resumes the worker, then `wake`. Either way no other item runs
    // and no host task is polled in between: the other instance's
    // callback, which yields for ever, and its host task, which wakes
    // itself on every poll, note nothing between `wake`'s `2` and its
    // `4`. Without a provider the worker waits on the real stack, and
    // nothing can switch to it.
    let engine = engine(true);
    if !has_provider(&engine) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension,
        "the web lane runs the host-suspension provider"
    );
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    {
        let notes = notes.clone();
        linker
            .root()
            .func_wrap_concurrent("tick", move |_accessor: &Accessor<()>, (): ()| Ticks {
                notes: notes.clone(),
            })
            .expect("the registration");
    }
    let (mut store, instance) = instantiate(&engine, &linker, WAKES_ITS_OWN_WORKER).await;

    let setup = func(&instance, "setup")
        .call(&mut store, &[])
        .await
        .expect("`setup` returns once its worker has suspended");
    assert_eq!(setup.as_ref(), [Val::U32(0)]);
    {
        // The spinner stays in the store once its call is dropped.
        let spin = func(&instance, "spin");
        let mut spin = Box::pin(spin.call(&mut store, &[]));
        assert!(poll_once(&mut spin).is_pending(), "`spin` never returns");
    }
    let woken = func(&instance, "wake")
        .call(&mut store, &[])
        .await
        .expect("`wake` returns once its worker has run");

    assert_eq!(woken.as_ref(), [Val::U32(7)]);
    let notes = notes.lock().expect("notes").clone();
    let begun = notes
        .iter()
        .position(|note| *note == 2)
        .unwrap_or_else(|| panic!("`wake` began: {notes:?}"));
    assert_eq!(
        notes.get(begun..begun + 3),
        Some(&[2, 3, 4][..]),
        "the worker ran inside `wake`'s call, and nothing else ran and no \
         host task was polled until it stopped and `wake` returned: {notes:?}"
    );
    assert!(
        notes[..begun].contains(&100) && notes[..begun].contains(&200),
        "the other instance was busy before `wake` began: {notes:?}"
    );
}

/// Two component instances, as in [`WAKES_ITS_OWN_WORKER`], where the
/// synchronous export blocks rather than switching.
///
/// In the first, `setup` starts a worker, which suspends with
/// `thread.suspend`, and then suspends itself once it has noted `1`
/// and resolved its task. `wake` notes `2`, makes the worker ready
/// with `thread.resume-later`, and yields once. The worker runs in
/// that yield: it notes `3` and ends. `wake` then notes `4` and returns
/// 7, or traps when the worker has not run.
///
/// In the second, the stackful `spin` lowers the host's `tick`
/// asynchronously and then notes `200` and yields for ever, so its
/// thread is ready whenever it is suspended. `tick`'s host task notes
/// `100` on each poll and wakes itself on each poll.
const YIELDS_TO_ITS_OWN_WORKER: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (import "tick" (func $tick async))
      (component $own
        (import "note" (func $note (param "step" u32)))
        (core module $libc (table (export "__indirect_function_table") 1 funcref))
        (core instance $libc (instantiate $libc))
        (core func $note (canon lower (func $note)))
        (core func $task-return (canon task.return (result u32)))
        (core type $start-ty (func (param i32)))
        (alias core export $libc "__indirect_function_table" (core table $table))
        (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
        (core func $resume-later (canon thread.resume-later))
        (core func $yield (canon thread.yield))
        (core func $suspend (canon thread.suspend))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
          (import "" "thread.resume-later" (func $resume-later (param i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (import "" "thread.suspend" (func $suspend (result i32)))
          (import "libc" "__indirect_function_table" (table 1 funcref))
          (global $worker (mut i32) (i32.const 0))
          (global $done (mut i32) (i32.const 0))
          (func $work (param i32)
            (drop (call $suspend))
            (call $note (i32.const 3))
            (global.set $done (i32.const 1)))
          (elem (table 0) (i32.const 0) func $work)
          (func (export "setup")
            (global.set $worker (call $new-indirect (i32.const 0) (i32.const 0)))
            (call $resume-later (global.get $worker))
            (drop (call $yield))
            (call $note (i32.const 1))
            (call $task-return (i32.const 0))
            (drop (call $suspend)))
          (func (export "wake") (result i32)
            (call $note (i32.const 2))
            (call $resume-later (global.get $worker))
            (drop (call $yield))
            (if (i32.eqz (global.get $done)) (then unreachable))
            (call $note (i32.const 4))
            (i32.const 7)))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "note" (func $note))
            (export "task.return" (func $task-return))
            (export "thread.new-indirect" (func $new-indirect))
            (export "thread.resume-later" (func $resume-later))
            (export "thread.yield" (func $yield))
            (export "thread.suspend" (func $suspend))))
          (with "libc" (instance $libc))))
        (func (export "setup") async (result u32)
          (canon lift (core func $i "setup") async))
        (func (export "wake") (result u32)
          (canon lift (core func $i "wake"))))
      (component $other
        (import "note" (func $note (param "step" u32)))
        (import "tick" (func $tick async))
        (core func $note (canon lower (func $note)))
        (core func $tick (canon lower (func $tick) async))
        (core func $yield (canon thread.yield))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "tick" (func $tick (result i32)))
          (import "" "thread.yield" (func $yield (result i32)))
          (func (export "spin")
            (drop (call $tick))
            (loop $again
              (call $note (i32.const 200))
              (drop (call $yield))
              (br $again))))
        (core instance $i (instantiate $m
          (with "" (instance
            (export "note" (func $note))
            (export "tick" (func $tick))
            (export "thread.yield" (func $yield))))))
        (func (export "spin") async
          (canon lift (core func $i "spin") async)))
      (instance $a (instantiate $own (with "note" (func $note))))
      (instance $b (instantiate $other (with "note" (func $note)) (with "tick" (func $tick))))
      (export "setup" (func $a "setup"))
      (export "wake" (func $a "wake"))
      (export "spin" (func $b "spin")))
    "#
);

#[wcmp_macros::test]
async fn it_resumes_a_suspended_thread_of_its_own_instance_from_a_blocked_synchronous_export_and_nothing_else()
 {
    // `wake`'s task must not block, so its yield takes the nested turn
    // held to its own instance. The worker is suspended in the
    // provider and `wake` made it ready, so the turn resumes it
    // through the provider from inside the yield, as the reference's
    // `canon_lift` runs a ready thread of the instance once a thread
    // of a sync-typed call blocks. Under the stack-switching provider
    // the resume is made in place. The host-suspension provider cannot resume a
    // stack from inside a call, so `wake`'s own thread suspends as
    // well, and the scheduler resumes the worker, then `wake`. Either
    // way nothing else runs and no host task is polled in between: the
    // other instance's thread, suspended in the provider and ready
    // whenever it yields, and its host task, which wakes itself on
    // every poll, note nothing between `wake`'s `2` and its `4`.
    // Without a provider the worker waits on the real stack, and
    // nothing can resume it from inside the call.
    let engine = engine(true);
    if !has_provider(&engine) {
        return;
    }
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    {
        let notes = notes.clone();
        linker
            .root()
            .func_wrap_concurrent("tick", move |_accessor: &Accessor<()>, (): ()| Ticks {
                notes: notes.clone(),
            })
            .expect("the registration");
    }
    let (mut store, instance) = instantiate(&engine, &linker, YIELDS_TO_ITS_OWN_WORKER).await;

    let setup = func(&instance, "setup")
        .call(&mut store, &[])
        .await
        .expect("`setup` returns once its worker has suspended");
    assert_eq!(setup.as_ref(), [Val::U32(0)]);
    {
        // The spinner stays in the store once its call is dropped.
        let spin = func(&instance, "spin");
        let mut spin = Box::pin(spin.call(&mut store, &[]));
        assert!(poll_once(&mut spin).is_pending(), "`spin` never returns");
    }
    let woken = func(&instance, "wake")
        .call(&mut store, &[])
        .await
        .expect("`wake` returns once its worker has run");

    assert_eq!(woken.as_ref(), [Val::U32(7)]);
    let notes = notes.lock().expect("notes").clone();
    let begun = notes
        .iter()
        .position(|note| *note == 2)
        .unwrap_or_else(|| panic!("`wake` began: {notes:?}"));
    assert_eq!(
        notes.get(begun..begun + 3),
        Some(&[2, 3, 4][..]),
        "the worker ran inside `wake`'s call, and nothing else ran and no \
         host task was polled until it ended and `wake` returned: {notes:?}"
    );
    assert!(
        notes[..begun].contains(&100) && notes[..begun].contains(&200),
        "the other instance was busy before `wake` began: {notes:?}"
    );
}

/// Host data that notes `500` when its store frees it.
#[cfg(target_arch = "wasm32")]
struct NotesItsDrop(Notes);

#[cfg(target_arch = "wasm32")]
impl Drop for NotesItsDrop {
    fn drop(&mut self) {
        self.0.lock().expect("notes").push(500);
    }
}

/// Let the browser run microtasks until `notes` holds `note`, or give
/// up after enough of them for any settled promise to have been seen.
#[cfg(target_arch = "wasm32")]
async fn run_microtasks_until(notes: &Notes, note: u32) {
    for _ in 0..64 {
        if notes.lock().expect("notes").contains(&note) {
            return;
        }
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(
            &wasm_bindgen::JsValue::UNDEFINED,
        ))
        .await
        .expect("a resolved promise");
    }
}

#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_runs_nothing_in_a_store_dropped_while_a_resumed_thread_has_yet_to_run() {
    // The first poll of the call suspends the export's thread on
    // `hold`, polls `hold` to its end, and resumes the thread, which
    // the browser runs on a microtask: the turn ends there, and the
    // poll returns pending. The test drops the store before the
    // microtask runs. The thread runs all the same, and finds the
    // store dropped: its shim traps before the guest goes on, so the
    // guest never drops its handle and no destructor runs. The store
    // is freed then, and its host data with it, which notes `500`. A
    // later store of the same engine runs the export to its end.
    let engine = engine(true);
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension
    );
    let notes: Notes = Arc::default();
    let mut linker = noting::<NotesItsDrop>(&engine, &notes);
    linker
        .root()
        .func_wrap_concurrent("hold", |_accessor: &Accessor<NotesItsDrop>, (): ()| {
            PendingOnce { polled: false }
        })
        .expect("the registration");
    let component = Component::new(&engine, HOLDS_A_HANDLE_ACROSS_A_BLOCK)
        .await
        .expect("component parses");

    {
        let mut store = Store::new(&engine, NotesItsDrop(notes.clone())).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("instantiate");
        let run = func(&instance, "run");
        let first = {
            let mut call = Box::pin(run.call(&mut store, &[]));
            poll_once(&mut call)
        };
        assert!(first.is_pending(), "the resumed thread runs on a microtask");
        assert!(
            store.internal().context().internal().deferred_busy(),
            "the turn stopped for the resumed thread"
        );
        assert!(notes.lock().expect("notes").is_empty());
        drop(store);
    }
    run_microtasks_until(&notes, 500).await;
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![500],
        "the thread resumed before the drop ran no guest code, and the store was freed"
    );

    let mut store = Store::new(&engine, NotesItsDrop(notes.clone())).expect("a later store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let result = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect("the export runs to its end in the later store");
    assert_eq!(result.as_ref(), [Val::U32(7)]);
    assert_eq!(notes.lock().expect("notes").clone(), vec![500, 99]);
}

#[wcmp_macros::test]
async fn it_takes_the_stop_of_a_resume_whose_driver_was_dropped_on_the_next_call() {
    // The first poll of the call resumes the export's thread, which
    // the browser runs on a microtask, and the test drops the call
    // there. The resume stays with the store: the next call's driver
    // takes the thread's stop before it does anything else, and runs
    // its own thread to its end. The first thread went on in the
    // meantime, so both threads dropped their handle. Wasmi runs a
    // resume to its stop inside the poll that makes it, so there no
    // resume outlives its call: the first call ends in its first poll,
    // and the next one runs as it would anyway. The test below drops a
    // call whose thread waits, on every backend.
    let engine = suspending_engine();
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension
    );
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    linker
        .root()
        .func_wrap_concurrent("hold", |_accessor: &Accessor<()>, (): ()| PendingOnce {
            polled: false,
        })
        .expect("the registration");
    let (mut store, instance) = instantiate(&engine, &linker, HOLDS_A_HANDLE_ACROSS_A_BLOCK).await;
    let run = func(&instance, "run");

    {
        let mut call = Box::pin(run.call(&mut store, &[]));
        let first = poll_once(&mut call);
        #[cfg(target_arch = "wasm32")]
        assert!(first.is_pending(), "the resumed thread runs on a microtask");
        #[cfg(not(target_arch = "wasm32"))]
        drop(first);
    }
    #[cfg(target_arch = "wasm32")]
    assert!(
        store.internal().context().internal().deferred_busy(),
        "the resume outlived the call that made it"
    );

    let result = run
        .call(&mut store, &[])
        .await
        .expect("the next call runs to its end");
    assert_eq!(result.as_ref(), [Val::U32(7)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![99, 99],
        "the thread whose call was dropped ran on, and so did the next one"
    );
}

#[wcmp_macros::test]
async fn it_runs_the_thread_of_a_dropped_call_in_a_later_call() {
    // The first poll of the call suspends the export's thread on
    // `hold`, which stays pending until the test releases it, and the
    // test drops the call there, on every backend that declares host
    // suspension. Dropping the call cancels nothing: its task stays in
    // the store. The next call's driver runs until its own task
    // returns, which on every provider is before the first thread goes
    // on, and a later call's driver runs the first thread on to its end,
    // where it drops its handle.
    let engine = suspending_engine();
    assert_eq!(
        engine.suspend_provider(),
        SuspendProviderKind::HostSuspension
    );
    let notes: Notes = Arc::default();
    let mut linker = noting(&engine, &notes);
    let gate: Arc<Mutex<Gate>> = Arc::default();
    let held = gate.clone();
    linker
        .root()
        .func_wrap_concurrent("hold", move |_accessor: &Accessor<()>, (): ()| {
            let gated = Gated {
                gate: held.clone(),
                value: 0,
            };
            async move { gated.await.map(|_| ()) }
        })
        .expect("the registration");
    let (mut store, instance) = instantiate(&engine, &linker, HOLDS_A_HANDLE_ACROSS_A_BLOCK).await;
    let run = func(&instance, "run");

    {
        let mut call = Box::pin(run.call(&mut store, &[]));
        assert!(
            poll_once(&mut call).is_pending(),
            "the thread waits on `hold`"
        );
    }
    assert!(notes.lock().expect("notes").is_empty());
    let waker = {
        let mut gate = gate.lock().expect("gate");
        gate.released = true;
        gate.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }

    let result = run
        .call(&mut store, &[])
        .await
        .expect("the next call runs to its end");
    assert_eq!(result.as_ref(), [Val::U32(7)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![99],
        "the next call's own thread ran to its end"
    );

    let result = run
        .call(&mut store, &[])
        .await
        .expect("a later call runs to its end");
    assert_eq!(result.as_ref(), [Val::U32(7)]);
    assert_eq!(
        notes.lock().expect("notes").clone(),
        vec![99, 99, 99],
        "the thread whose call was dropped ran on, and so did the later one"
    );
}

/// Stackful exports that fail before and after their thread first
/// suspends: with a trap of their own, or with the failure of a host
/// import they call. `fail` is the host import.
const FAILS_AROUND_A_SUSPENSION: &[u8] = component!(
    r#"
    (component
      (import "fail" (func $fail))
      (core func $fail (canon lower (func $fail)))
      (core func $yield (canon thread.yield))
      (core module $m
        (import "" "fail" (func $fail))
        (import "" "yield" (func $yield (result i32)))
        (func (export "trap-early") unreachable)
        (func (export "trap-late") (drop (call $yield)) unreachable)
        (func (export "fail-early") (call $fail))
        (func (export "fail-late") (drop (call $yield)) (call $fail)))
      (core instance $i (instantiate $m (with "" (instance
        (export "fail" (func $fail))
        (export "yield" (func $yield))))))
      (func (export "trap-early") async (result u32)
        (canon lift (core func $i "trap-early") async))
      (func (export "trap-late") async (result u32)
        (canon lift (core func $i "trap-late") async))
      (func (export "fail-early") async (result u32)
        (canon lift (core func $i "fail-early") async))
      (func (export "fail-late") async (result u32)
        (canon lift (core func $i "fail-late") async)))
    "#
);

/// A caller whose asynchronous lower starts a callee of a second
/// component as a nested start, and whose callee traps before it first
/// suspends.
const STARTS_A_CALLEE_THAT_TRAPS: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (component $callee
        (core module $m
          (func (export "work") unreachable))
        (core instance $i (instantiate $m))
        (func (export "work") async (result u32)
          (canon lift (core func $i "work") async)))
      (component $caller
        (import "note" (func $note (param "step" u32)))
        (import "work" (func $work async (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $note (canon lower (func $note)))
        (core func $work (canon lower (func $work) async (memory (core memory $libc "mem"))))
        (core module $m
          (import "" "note" (func $note (param i32)))
          (import "" "work" (func $work (param i32) (result i32)))
          (func (export "run")
            (drop (call $work (i32.const 0)))
            (call $note (i32.const 1))))
        (core instance $i (instantiate $m (with "" (instance
          (export "note" (func $note))
          (export "work" (func $work))))))
        (func (export "run") async (result u32)
          (canon lift (core func $i "run") async)))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller
        (with "note" (func $note))
        (with "work" (func $a "work"))))
      (export "run" (func $b "run")))
    "#
);

#[wcmp_macros::test]
async fn it_fails_a_thread_with_its_own_trap_or_host_error_before_and_after_it_suspends() {
    // A thread that fails before it first suspends fails its call with
    // its own trap, as one that fails after a resumption does. The
    // host-suspension provider starts a thread from a turn as the
    // store's flight, which the driver awaits, so the trap reaches the
    // scheduler with its reason, even in the browser, which reports it
    // on a microtask. A host import's error is the failure
    // either way, and one thread's host error does not leak into
    // another's failure. A failure is a trap, and a trap poisons the
    // store, so each call runs in a store of its own.
    let engine = engine(true);
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("fail", |_: HostCall<'_, ()>, (): ()| -> Result<(), Error> {
            Err(Error::Unsupported {
                feature: "the host refused".to_owned(),
            })
        })
        .expect("the registration");

    for (name, expected) in [
        ("trap-early", "unreachable"),
        ("fail-early", "the host refused"),
        ("trap-late", "unreachable"),
        ("fail-late", "the host refused"),
    ] {
        let (mut store, instance) = instantiate(&engine, &linker, FAILS_AROUND_A_SUSPENSION).await;
        let err = func(&instance, name)
            .call(&mut store, &[])
            .await
            .expect_err("the export fails");
        let message = chain(&err);
        assert!(
            message.contains(expected),
            "`{name}` fails with `{expected}`, got {message}"
        );
        if expected == "unreachable" {
            assert!(
                !message.contains("the host refused"),
                "`{name}` fails with its own trap, not a host error: {message}"
            );
        }
        if has_provider(&engine) {
            assert_eq!(parked_threads(&mut store), 0, "no thread is left suspended");
        }
    }
}

#[wcmp_macros::test]
async fn it_fails_the_caller_of_a_nested_start_whose_callee_traps_before_it_suspends() {
    // The caller's asynchronous lower starts the callee from inside
    // the start intrinsic, and the callee traps at once. The trap
    // fails the caller's call with the callee's own trap, and the
    // caller never gets past the lower. Under the host-suspension provider the
    // intrinsic learns of the trap only on a microtask, so the caller's
    // thread suspends in the intrinsic's shim until the scheduler has
    // the trap, and fails there.
    let engine = engine(true);
    let notes: Notes = Arc::default();
    let linker = noting(&engine, &notes);
    let (mut store, instance) = instantiate(&engine, &linker, STARTS_A_CALLEE_THAT_TRAPS).await;

    let err = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect_err("the callee's trap fails the caller's call");

    let message = chain(&err);
    assert!(
        message.contains("unreachable"),
        "the caller fails with the callee's trap, got {message}"
    );
    assert!(
        notes.lock().expect("notes").is_empty(),
        "the caller never got past its lower"
    );
    if has_provider(&engine) {
        assert_eq!(parked_threads(&mut store), 0, "no thread is left suspended");
    }
}

/// A synchronous export, `yield-then-trap`, that starts a thread whose
/// entry traps and yields to it with `thread.yield-then-resume`. The
/// export's thread notes `1` once it goes on after the yield, which it
/// never may: the trap of the thread it yielded to poisons the store
/// first.
const YIELDS_TO_A_THREAD_THAT_TRAPS: &[u8] = component!(
    r#"
    (component
      (import "note" (func $note (param "step" u32)))
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core func $note (canon lower (func $note)))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core module $m
        (import "" "note" (func $note (param i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (func $trap (param i32) unreachable)
        (elem (table 0) (i32.const 0) func $trap)
        (func (export "yield-then-trap")
          (drop (call $yield-then-resume
            (call $new-indirect (i32.const 0) (i32.const 0))))
          (call $note (i32.const 1))))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "note" (func $note))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.yield-then-resume" (func $yield-then-resume))))
        (with "libc" (instance $libc))))
      (func (export "yield-then-trap")
        (canon lift (core func $i "yield-then-trap"))))
    "#
);

#[wcmp_macros::test]
async fn it_resumes_no_thread_that_yielded_before_a_trap_in_a_later_driver_of_the_poisoned_store() {
    // Under a provider the export's thread suspends in the provider as
    // it yields, and the store keeps it to take back, still ready, once
    // the thread it yielded to stops. That thread traps, which poisons
    // the store and ends the call before the take-back. A later driver
    // runs only host work: it must not resume the export's thread, and
    // nothing the store kept may hold it up. With no provider the
    // started thread runs above the yield on the real stack, and its
    // trap unwinds the export's thread with it.
    for engine in [engine(true), engine(false), suspending_engine()] {
        let notes: Notes = Arc::default();
        let linker = noting(&engine, &notes);
        let (mut store, instance) =
            instantiate(&engine, &linker, YIELDS_TO_A_THREAD_THAT_TRAPS).await;

        let err = func(&instance, "yield-then-trap")
            .call(&mut store, &[])
            .await
            .expect_err("the thread the export yielded to traps");
        let message = chain(&err);
        assert!(
            message.contains("unreachable"),
            "the call fails with the started thread's trap, got {message}"
        );

        let polls = store
            .run_concurrent(async |_accessor| {
                let mut polls = 0;
                core::future::poll_fn(|context| {
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
        assert!(
            notes.lock().expect("notes").is_empty(),
            "no driver of the poisoned store resumed the export's thread, under {:?}",
            engine.suspend_provider()
        );
    }
}
