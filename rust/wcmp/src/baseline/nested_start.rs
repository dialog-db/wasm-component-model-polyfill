// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the cause of a block above a nested start.
//!
//! A start intrinsic runs an `async`-typed callee from inside its own
//! frame, on the real stack above the caller that lowered the call.
//! The reference runs that callee on a stack of its own and returns
//! to the caller's lower when the callee blocks. After an
//! asynchronous lower the caller's own code goes on from there, so a
//! callee that waits for work only its caller does is released once
//! the caller gets to it. Without a stack switch the caller cannot go
//! on: its frame is below the callee's, and it moves only when the
//! callee returns. The block then fails with the stack-switch cause
//! rather than the deadlock cause, because the store being idle does
//! not mean nothing can move. Only the target's capability is
//! missing.
//!
//! A synchronous lower reaches a nested start too. Once the callee
//! has called `task.return`, the lower would return its result and
//! the caller's own code would go on, as after an asynchronous lower.
//! Before that, the caller would get control back only to wait for
//! the callee's result. That wait runs the same ready work the
//! callee's own block ran, so the caller releases nothing and the
//! same block is a deadlock, as Wasmtime reports it.
//!
//! The callee reads a future synchronously in its first core
//! function, and only the caller writes that future, after the lower
//! returns. The store marks each nested start on its stack of current
//! scopes for as long as the callee runs, and the tests check that
//! the mark leaves with the failure.

#![cfg(test)]

use crate::store::StoreInternalExt;
use crate::{
    Component, Engine, EngineConfig, Error, Instance, Linker, SchedulerCause, Store,
    SuspendProviderKind,
};
use wcmp_macros::component;

/// A callee that reads the future it is handed synchronously, which
/// blocks until someone writes it, and a caller that hands it the
/// readable end and writes the future once the lower returns.
///
/// `run-async` lowers `take` asynchronously and `run-sync` lowers it
/// synchronously. `run-sync-returned` lowers `take-returned`
/// synchronously, whose callee calls `task.return` before it reads.
/// Every caller export is `async`-typed with a synchronous lift, so
/// the caller is allowed to block and the cannot-block rule does not
/// decide the cause. Under a stack switch the asynchronous lower
/// would return `STARTED`, and the synchronous lower of
/// `take-returned` would return the result the callee gave, so in
/// both the caller would write 42 and the callee would read it. The
/// synchronous lower of `take` would leave the caller waiting on a
/// callee that waits on it.
const READS_WHAT_ONLY_THE_CALLER_WRITES: &[u8] = component!(
    r#"
    (component
      (component $callee
        (type $f (future u32))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $read (canon future.read $f (memory (core memory $libc "mem"))))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "future.read" (func $read (param i32 i32) (result i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "take") (param i32) (result i32)
            (drop (call $read (local.get 0) (i32.const 16)))
            (call $task-return (i32.load (i32.const 16)))
            (i32.const 0))
          (func (export "take-returned") (param i32) (result i32)
            (call $task-return (i32.const 0))
            (drop (call $read (local.get 0) (i32.const 16)))
            (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "future.read" (func $read))
          (export "task.return" (func $task-return))))))
        (func (export "take") async (param "f" $f) (result u32)
          (canon lift (core func $i "take") async (callback (core func $i "cb"))))
        (func (export "take-returned") async (param "f" $f) (result u32)
          (canon lift (core func $i "take-returned") async (callback (core func $i "cb")))))
      (component $caller
        (type $f (future u32))
        (import "take" (func $take async (param "f" $f) (result u32)))
        (import "take-returned" (func $take-returned async (param "f" $f) (result u32)))
        (core module $libc (memory (export "mem") 1))
        (core instance $libc (instantiate $libc))
        (core func $future-new (canon future.new $f))
        (core func $write (canon future.write $f async (memory (core memory $libc "mem"))))
        (core func $take-async
          (canon lower (func $take) async (memory (core memory $libc "mem"))))
        (core func $take-sync (canon lower (func $take)))
        (core func $take-returned-sync (canon lower (func $take-returned)))
        (core module $m
          (import "" "mem" (memory 1))
          (import "" "future.new" (func $future-new (result i64)))
          (import "" "future.write" (func $write (param i32 i32) (result i32)))
          (import "" "take-async" (func $take-async (param i32 i32) (result i32)))
          (import "" "take-sync" (func $take-sync (param i32) (result i32)))
          (import "" "take-returned-sync" (func $take-returned-sync (param i32) (result i32)))
          (func $write-42 (param $ends i64)
            (i32.store (i32.const 0) (i32.const 42))
            (drop (call $write
              (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32)))
              (i32.const 0))))
          (func (export "run-async") (result i32)
            (local $ends i64)
            (local.set $ends (call $future-new))
            (drop (call $take-async (i32.wrap_i64 (local.get $ends)) (i32.const 8)))
            (call $write-42 (local.get $ends))
            (i32.const 7))
          (func (export "run-sync") (result i32)
            (local $ends i64)
            (local.set $ends (call $future-new))
            (drop (call $take-sync (i32.wrap_i64 (local.get $ends))))
            (call $write-42 (local.get $ends))
            (i32.const 7))
          (func (export "run-sync-returned") (result i32)
            (local $ends i64)
            (local.set $ends (call $future-new))
            (drop (call $take-returned-sync (i32.wrap_i64 (local.get $ends))))
            (call $write-42 (local.get $ends))
            (i32.const 7)))
        (core instance $i (instantiate $m (with "" (instance
          (export "mem" (memory $libc "mem"))
          (export "future.new" (func $future-new))
          (export "future.write" (func $write))
          (export "take-async" (func $take-async))
          (export "take-sync" (func $take-sync))
          (export "take-returned-sync" (func $take-returned-sync))))))
        (func (export "run-async") async (result u32) (canon lift (core func $i "run-async")))
        (func (export "run-sync") async (result u32) (canon lift (core func $i "run-sync")))
        (func (export "run-sync-returned") async (result u32)
          (canon lift (core func $i "run-sync-returned"))))
      (instance $a (instantiate $callee))
      (instance $b (instantiate $caller
        (with "take" (func $a "take"))
        (with "take-returned" (func $a "take-returned"))))
      (export "run-async" (func $b "run-async"))
      (export "run-sync" (func $b "run-sync"))
      (export "run-sync-returned" (func $b "run-sync-returned")))
    "#
);

const STACK_SWITCH: &str =
    "blocking here requires a stack switch, but this thread cannot switch its stack";

/// Instantiate `bytes` into a store of its own, with no imports. The
/// engine accepts the built-ins the Component Model gates behind its
/// "more async built-ins" feature, of which a synchronous
/// `future.read` is one. The cause of a failed block is the
/// fallback's, so the engine turns the suspend provider off: under a
/// provider the callee runs on a stack of its own and suspends, and
/// the caller below it goes on.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    instantiate_with(bytes, false).await
}

/// Instantiate `bytes` as [`instantiate`] does, with the suspend
/// provider allowed or turned off.
async fn instantiate_with(bytes: &[u8], provider: bool) -> (Store<()>, Instance) {
    let mut config = EngineConfig::new();
    config.wasm_component_model_more_async_builtins(true);
    config.suspend_provider(provider);
    let engine = Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
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

/// How many entries the store's stack of current scopes holds,
/// nested-start marks included.
fn scope_depth(store: &Store<()>) -> usize {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .scopes()
        .len()
}

/// Call `name` and hand back the whole message it fails with.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str) -> String {
    chain(&call_error(store, instance, name).await)
}

/// Call `name` with no arguments and hand back the error it failed
/// with.
async fn call_error(store: &mut Store<()>, instance: &Instance, name: &str) -> Error {
    let func = instance.get_func(name).expect("the caller's export");
    func.call(store, &[])
        .await
        .map(|values| format!("the call returned {values:?}"))
        .expect_err("the callee waits for its caller, which is below it on the stack")
}

#[wcmp_macros::test]
async fn it_fails_a_callee_an_async_lower_started_that_waits_on_its_caller_with_the_stack_switch_cause()
 {
    // The asynchronous start runs the callee from inside the lower,
    // and the callee's read blocks there. The nested turn finds
    // nothing to run and no host task, so the store is idle, but the
    // caller below the start would write the future once a stack
    // switch returned control to it.
    let (mut store, instance) = instantiate(READS_WHAT_ONLY_THE_CALLER_WRITES).await;
    let error = call_error(&mut store, &instance, "run-async").await;
    let message = chain(&error);
    assert!(
        message.contains(STACK_SWITCH),
        "expected the stack-switch cause, got {message}"
    );
    assert!(
        !message.contains("deadlock detected"),
        "a frame below the block could still move, so this is not a deadlock: {message}"
    );
    // The start intrinsic ran the callee whose block raised the cause,
    // and the host gets the cause itself back.
    assert!(
        matches!(error, Error::Scheduler(SchedulerCause::StackSwitchNeeded)),
        "expected the stack-switch cause as a value, got {error:?}"
    );
    assert_eq!(
        scope_depth(&store),
        0,
        "the nested-start mark left the stack with the failure"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_callee_a_sync_lower_started_that_waits_on_its_caller_with_the_deadlock_cause() {
    // The synchronous start runs the callee from inside the lower
    // too, and the callee's read blocks above the caller in the same
    // way. A stack switch would return control to the lower only for
    // the caller to wait on the callee, so nothing below the block
    // can move. Wasmtime reports the same shape as a deadlock, in
    // `wasmtime/async/future-read.wast` of the corpus.
    let (mut store, instance) = instantiate(READS_WHAT_ONLY_THE_CALLER_WRITES).await;
    let message = call_trap(&mut store, &instance, "run-sync").await;
    assert!(
        message.contains("deadlock detected: event loop cannot make further progress"),
        "expected the deadlock cause, got {message}"
    );
    assert_eq!(
        scope_depth(&store),
        0,
        "the nested-start mark left the stack with the failure"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_callee_that_returned_to_a_sync_lower_and_waits_on_its_caller_with_the_stack_switch_cause()
 {
    // The callee calls `task.return` and then blocks on the future.
    // A stack switch would return control to the synchronous lower,
    // which would hand the caller the result, and the caller's own
    // code would write the future. So a frame below the block could
    // still move, as it can after an asynchronous lower.
    let (mut store, instance) = instantiate(READS_WHAT_ONLY_THE_CALLER_WRITES).await;
    let message = call_trap(&mut store, &instance, "run-sync-returned").await;
    assert!(
        message.contains(STACK_SWITCH),
        "expected the stack-switch cause, got {message}"
    );
    assert_eq!(
        scope_depth(&store),
        0,
        "the nested-start mark left the stack with the failure"
    );
}

/// A caller that lowers the callee's `boom` asynchronously, so a
/// start intrinsic runs the callee from inside its own frame, and a
/// callee that calls the host function `panic` from its first core
/// function.
#[cfg(not(target_arch = "wasm32"))]
const CALLEE_PANICS: &[u8] = component!(
    r#"
    (component
      (import "panic" (func $panic))
      (component $callee
        (import "panic" (func $panic))
        (core func $panic (canon lower (func $panic)))
        (core module $m
          (import "" "panic" (func $panic))
          (func (export "boom") (result i32) (call $panic) (i32.const 0))
          (func (export "cb") (param i32 i32 i32) (result i32) unreachable))
        (core instance $i (instantiate $m (with "" (instance
          (export "panic" (func $panic))))))
        (func (export "boom") async
          (canon lift (core func $i "boom") async (callback (core func $i "cb")))))
      (component $caller
        (import "boom" (func $boom async))
        (core func $boom (canon lower (func $boom) async))
        (core module $m
          (import "" "boom" (func $boom (result i32)))
          (func (export "go") (result i32) (drop (call $boom)) (i32.const 7)))
        (core instance $i (instantiate $m (with "" (instance
          (export "boom" (func $boom))))))
        (func (export "go") async (result u32) (canon lift (core func $i "go"))))
      (instance $a (instantiate $callee (with "panic" (func $panic))))
      (instance $b (instantiate $caller (with "boom" (func $a "boom"))))
      (export "go" (func $b "go")))
    "#
);

/// A panic is an abort in the browser, so only a native build can
/// unwind one, and only over Wasmtime: Wasmi does not unwind a host
/// function's panic through guest code, so the test names Wasmtime
/// whatever backend its lane runs.
#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_leaves_no_nested_start_mark_when_the_callee_a_start_ran_panics() {
    use core::future::Future;
    use core::task::{Context, Waker};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};

    use crate::HostCall;
    use crate::concurrency::Scope;
    use crate::resource::HandleTables;
    use crate::store::StoreContextInternalExt;

    /// How many nested-start marks the stack of scopes holds.
    fn nested_start_marks(tables: &Mutex<HandleTables>) -> usize {
        tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tasks
            .scopes()
            .iter()
            .filter(|scope| matches!(scope, Scope::NestedStart { .. }))
            .count()
    }

    let mut config = EngineConfig::new();
    config.suspend_provider(false);
    let engine = Engine::with_backend(crate::runtime_layer::test_wasmtime_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine");
    let component = Component::new(&engine, CALLEE_PANICS)
        .await
        .expect("component parses");
    // How many nested-start marks were on the stack while the callee
    // ran, read by the host function before it panics.
    let marked = Arc::new(AtomicUsize::new(0));
    let seen = marked.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "panic",
            move |mut call: HostCall<'_, ()>, (): ()| -> crate::Result<()> {
                let marks = nested_start_marks(call.store().internal_ref().tables());
                seen.store(marks, Ordering::SeqCst);
                panic!("the host function panics")
            },
        )
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let go = instance.get_func("go").expect("the caller's export");

    // With no provider the whole call runs in its first poll, so the
    // panic unwinds out of that poll.
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let mut call = Box::pin(go.call(&mut store, &[]));
        let _ = call.as_mut().poll(&mut Context::from_waker(Waker::noop()));
    }));
    assert!(
        panicked.is_err(),
        "the host function's panic unwound the call"
    );
    assert_eq!(
        marked.load(Ordering::SeqCst),
        1,
        "the start intrinsic marked the stack while its callee ran"
    );
    assert_eq!(
        nested_start_marks(store.internal_ref().tables()),
        0,
        "the start took its mark off as the panic unwound"
    );
}

/// Whether the engine runs guest threads through a provider on this
/// target.
fn has_provider() -> bool {
    Engine::with_backend(crate::runtime_layer::test_backend())
        .expect("engine")
        .suspend_provider()
        != SuspendProviderKind::None
}

#[wcmp_macros::test]
async fn it_returns_from_callees_that_wait_on_their_caller_under_the_provider() {
    // Under a provider each callee runs on a stack of its own, so its
    // read suspends it and the start returns to the lower. After the
    // asynchronous lower, and after the synchronous lower of the
    // callee that returned first, the caller's own code goes on and
    // writes the future, and the callee resumes and reads it. Only
    // the synchronous lower of the callee that has not returned waits
    // on a callee that waits on it, which is a deadlock under a
    // provider too.
    if !has_provider() {
        return;
    }
    for name in ["run-async", "run-sync-returned"] {
        let (mut store, instance) = instantiate_with(READS_WHAT_ONLY_THE_CALLER_WRITES, true).await;
        let values = instance
            .get_func(name)
            .expect("the caller's export")
            .call(&mut store, &[])
            .await
            .unwrap_or_else(|error| panic!("`{name}` returns under a provider: {error:?}"));
        assert_eq!(values.as_ref(), [crate::Val::U32(7)], "`{name}`");
        assert_eq!(scope_depth(&store), 0, "`{name}` left no scope behind");
    }
    let (mut store, instance) = instantiate_with(READS_WHAT_ONLY_THE_CALLER_WRITES, true).await;
    let message = call_trap(&mut store, &instance, "run-sync").await;
    assert!(
        message.contains("deadlock detected: event loop cannot make further progress"),
        "expected the deadlock cause, got {message}"
    );
    assert_eq!(scope_depth(&store), 0);
}
