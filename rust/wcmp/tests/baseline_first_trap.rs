//! Baseline tests for where a trap surfaces.
//!
//! The first trap ends the driver that is polling, with that trap,
//! and poisons the store in the same step. A driver is `Func::call`,
//! `TypedFunc::call`, an instantiation, or `Store::run_concurrent`.
//! The rule holds whichever task the trap belongs to, and whether
//! the call that started that task is still polled, has returned, or
//! was dropped: a caller that already has its result never learns of
//! a later trap in its task, and the driver whose turn meets the trap
//! reports it. Each test then makes one more entry, which fails with
//! Wasmtime's cannot-enter trap because the store is poisoned.
//!
//! A host `async` function whose future fails is a trap of the guest
//! task that called it, and it poisons the store too, wherever that
//! guest task came from: a call from the host, a call from another
//! component, or a task that sits on the stack below another task's
//! nested turn.

#![cfg(test)]

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use wcmp::{
    Accessor, Component, Engine, EngineConfig, Error, Func, Instance, Linker, Store,
    SuspendProviderKind, TaskCause, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component of callback exports and two synchronous ones, which
/// imports the host `async` function `answer`.
///
/// - `resolve-then-trap` calls `task.return` with 6 and yields, and
///   its callback traps.
/// - `resolve-then-wait` calls `answer` through an asynchronous
///   lower, calls `task.return` with 7, and waits on the call's
///   subtask. Its callback ends the task.
/// - `call-and-wait` calls `answer` and waits on it, and resolves in
///   its callback with the answer.
/// - `drive` yields once, and resolves with 8 in its callback, so a
///   driver of it runs the work that other tasks left to give way.
/// - `ok` answers 9, and `trap` traps.
const CALLBACKS: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $answer'
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func $wait-on-answer (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $answer (i32.const 0)))
          (local.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (local.get $set))
          ;; Wait on the set.
          (i32.or (i32.const 2) (i32.shl (local.get $set) (i32.const 4))))
        (func (export "resolve-then-trap") (result i32)
          (call $task-return (i32.const 6))
          ;; Yield.
          (i32.const 1))
        (func (export "trap-callback") (param i32 i32 i32) (result i32)
          unreachable)
        (func (export "resolve-then-wait") (result i32)
          (local $wait i32)
          (local.set $wait (call $wait-on-answer))
          (call $task-return (i32.const 7))
          (local.get $wait))
        (func (export "exit-callback") (param i32 i32 i32) (result i32)
          ;; Exit.
          (i32.const 0))
        (func (export "call-and-wait") (result i32)
          (call $wait-on-answer))
        (func (export "answer-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.load (i32.const 0)))
          ;; Exit.
          (i32.const 0))
        (func (export "drive") (result i32)
          ;; Yield.
          (i32.const 1))
        (func (export "drive-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.const 8))
          ;; Exit.
          (i32.const 0))
        (func (export "ok") (result i32) (i32.const 9))
        (func (export "trap") unreachable))
      (core instance $i (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $answer'))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "resolve-then-trap") async (result u32)
        (canon lift (core func $i "resolve-then-trap") async
          (callback (core func $i "trap-callback"))))
      (func (export "resolve-then-wait") async (result u32)
        (canon lift (core func $i "resolve-then-wait") async
          (callback (core func $i "exit-callback"))))
      (func (export "call-and-wait") async (result u32)
        (canon lift (core func $i "call-and-wait") async
          (callback (core func $i "answer-callback"))))
      (func (export "drive") async (result u32)
        (canon lift (core func $i "drive") async
          (callback (core func $i "drive-callback"))))
      (func (export "ok") (result u32)
        (canon lift (core func $i "ok")))
      (func (export "trap")
        (canon lift (core func $i "trap"))))
    "#
);

/// A component whose one callback export, `$callee`'s `work`, calls
/// the host `async` function `answer` and waits on it, and whose
/// export `run` is `$caller`'s synchronous lift, which calls `work`
/// through a synchronous lower. `work`'s task is one the prepare
/// intrinsic creates for a call between two components, so no host
/// call owns it.
const GUEST_TO_GUEST: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (result u32)))

      (component $callee
        (import "answer" (func $answer async (result u32)))
        (core module $libc (memory (export "memory") 1))
        (core instance $libc (instantiate $libc))
        (core func $answer
          (canon lower (func $answer) async (memory (core memory $libc "memory"))))
        (core func $set-new (canon waitable-set.new))
        (core func $join (canon waitable.join))
        (core func $task-return (canon task.return (result u32)))
        (core module $m
          (import "libc" "memory" (memory 1))
          (import "" "answer" (func $answer (param i32) (result i32)))
          (import "" "waitable-set.new" (func $set-new (result i32)))
          (import "" "waitable.join" (func $join (param i32 i32)))
          (import "" "task.return" (func $task-return (param i32)))
          (func (export "work") (result i32)
            (local $set i32)
            (local.set $set (call $set-new))
            (call $join
              (i32.shr_u (call $answer (i32.const 0)) (i32.const 4))
              (local.get $set))
            ;; Wait on the set.
            (i32.or (i32.const 2) (i32.shl (local.get $set) (i32.const 4))))
          (func (export "callback") (param i32 i32 i32) (result i32)
            (call $task-return (i32.load (i32.const 0)))
            ;; Exit.
            (i32.const 0)))
        (core instance $m (instantiate $m
          (with "libc" (instance $libc))
          (with "" (instance
            (export "answer" (func $answer))
            (export "waitable-set.new" (func $set-new))
            (export "waitable.join" (func $join))
            (export "task.return" (func $task-return))))))
        (func (export "work") async (result u32)
          (canon lift (core func $m "work") async (callback (core func $m "callback")))))

      (component $caller
        (import "work" (func $work async (result u32)))
        (core func $work (canon lower (func $work)))
        (core module $m
          (import "" "work" (func $work (result i32)))
          (func (export "run") (result i32) (call $work)))
        (core instance $m (instantiate $m
          (with "" (instance (export "work" (func $work))))))
        (func (export "run") async (result u32)
          (canon lift (core func $m "run"))))

      (instance $callee (instantiate $callee (with "answer" (func $answer))))
      (instance $caller (instantiate $caller (with "work" (func $callee "work"))))
      (export "run" (func $caller "run")))
    "#
);

/// A component whose exports block, each lifted synchronously over an
/// `async` function type, which lets its task block.
///
/// - `block-on-answer` calls the host `async` function `answer`
///   through a synchronous lower and returns what it answered.
/// - `call-then-block` calls `answer` through an asynchronous lower,
///   which leaves the call running, and then calls the host `async`
///   function `block` through a synchronous lower.
/// - `block` calls `block` through a synchronous lower.
/// - `answer-then-trap` calls `answer` through a synchronous lower,
///   and traps once it has the answer.
/// - `ok` is lifted over a synchronous type, and answers 9.
const BLOCKS: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (result u32)))
      (import "block" (func $block async))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $answer-sync (canon lower (func $answer)))
      (core func $answer-async
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $block (canon lower (func $block)))
      (core module $m
        (import "" "answer-sync" (func $answer-sync (result i32)))
        (import "" "answer-async" (func $answer-async (param i32) (result i32)))
        (import "" "block" (func $block))
        (func (export "block-on-answer") (result i32)
          (call $answer-sync))
        (func (export "call-then-block")
          (drop (call $answer-async (i32.const 0)))
          (call $block))
        (func (export "block")
          (call $block))
        (func (export "answer-then-trap")
          (drop (call $answer-sync))
          unreachable)
        (func (export "ok") (result i32) (i32.const 9)))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "answer-sync" (func $answer-sync))
          (export "answer-async" (func $answer-async))
          (export "block" (func $block))))))
      (func (export "block-on-answer") async (result u32)
        (canon lift (core func $i "block-on-answer")))
      (func (export "call-then-block") async
        (canon lift (core func $i "call-then-block")))
      (func (export "block") async
        (canon lift (core func $i "block")))
      (func (export "answer-then-trap") async
        (canon lift (core func $i "answer-then-trap")))
      (func (export "ok") (result u32)
        (canon lift (core func $i "ok"))))
    "#
);

/// A component whose one instance holds a thread start function that
/// traps, at table entry 0.
///
/// - `spawn-ready` starts a thread that runs the trap and makes it
///   ready, and returns 1 before the thread runs.
/// - `spawn-held` starts a thread that runs the trap, leaves it
///   suspended, keeps its index, and returns 2.
/// - `switch-to-held` switches to the thread `spawn-held` left, from
///   a task of its own, and would return 3.
/// - `drive` yields once, and resolves with 4 in its callback.
/// - `spawn-ready-async` is lifted `async` with a callback. It starts
///   a thread that runs the trap and makes it ready, calls
///   `task.return` with 5, and exits, so its callback never runs.
const THREADS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))
      (core func $task-return (canon task.return (result u32)))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))
      (core func $suspend-then-resume (canon thread.suspend-then-resume))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "" "thread.suspend-then-resume"
          (func $suspend-then-resume (param i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (global $held (mut i32) (i32.const 0))
        (func $trap (param i32) unreachable)
        (elem (table 0) (i32.const 0) func $trap)
        (func (export "spawn-ready") (result i32)
          (call $resume-later (call $new-indirect (i32.const 0) (i32.const 0)))
          (i32.const 1))
        (func (export "spawn-held") (result i32)
          (global.set $held (call $new-indirect (i32.const 0) (i32.const 0)))
          (i32.const 2))
        (func (export "switch-to-held") (result i32)
          (drop (call $suspend-then-resume (global.get $held)))
          (i32.const 3))
        (func (export "drive") (result i32)
          ;; Yield.
          (i32.const 1))
        (func (export "drive-callback") (param i32 i32 i32) (result i32)
          (call $task-return (i32.const 4))
          ;; Exit.
          (i32.const 0))
        (func (export "spawn-ready-async") (result i32)
          (call $resume-later (call $new-indirect (i32.const 0) (i32.const 0)))
          (call $task-return (i32.const 5))
          ;; Exit.
          (i32.const 0))
        (func (export "never-callback") (param i32 i32 i32) (result i32)
          unreachable))
      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))
          (export "thread.suspend-then-resume" (func $suspend-then-resume))))
        (with "libc" (instance $libc))))
      (func (export "spawn-ready") (result u32)
        (canon lift (core func $i "spawn-ready")))
      (func (export "spawn-held") (result u32)
        (canon lift (core func $i "spawn-held")))
      (func (export "switch-to-held") (result u32)
        (canon lift (core func $i "switch-to-held")))
      (func (export "drive") async (result u32)
        (canon lift (core func $i "drive") async
          (callback (core func $i "drive-callback"))))
      (func (export "spawn-ready-async") async (result u32)
        (canon lift (core func $i "spawn-ready-async") async
          (callback (core func $i "never-callback")))))
    "#
);

/// What the host `async` functions of these tests fail with.
const HOST_FAILED: &str = "the host call never returned";

/// The message of Wasmtime's cannot-enter trap.
const CANNOT_ENTER: &str = "cannot enter component instance";

/// A future that is pending the first time it is polled and fails
/// afterwards: a host call the guest was told started, and that then
/// never returned. It wakes the waker it was polled with before it
/// parks, so the next turn polls it again.
struct FailsAfterAPendingPoll {
    polled: bool,
}

impl Future for FailsAfterAPendingPoll {
    type Output = Result<u32, Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.polled {
            return Poll::Ready(Err(host_failed()));
        }
        self.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// The gate a [`FailsWhenReleased`] waits behind: the test releases
/// it, and wakes the future that waits.
#[derive(Clone, Default)]
struct Release {
    released: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Release {
    /// Let the future fail, and wake it so that the next turn polls it.
    fn release(&self) {
        self.released.store(true, Ordering::Relaxed);
        if let Some(waker) = self.waker.lock().expect("the waker").take() {
            waker.wake();
        }
    }
}

/// A future that is pending until the test releases it, and fails on
/// the first poll after that.
struct FailsWhenReleased(Release);

impl Future for FailsWhenReleased {
    type Output = Result<u32, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0.released.load(Ordering::Relaxed) {
            return Poll::Ready(Err(host_failed()));
        }
        *self.0.waker.lock().expect("the waker") = Some(context.waker().clone());
        Poll::Pending
    }
}

/// A future that is pending until the test releases it, and answers 5
/// on the first poll after that.
struct AnswersWhenReleased(Release);

impl Future for AnswersWhenReleased {
    type Output = Result<u32, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0.released.load(Ordering::Relaxed) {
            return Poll::Ready(Ok(5));
        }
        *self.0.waker.lock().expect("the waker") = Some(context.waker().clone());
        Poll::Pending
    }
}

/// A future around a call future the closure of `run_concurrent`
/// awaits, which notes its drop: what the entry drops with the
/// closure.
struct NotesDrop<F> {
    future: Pin<Box<F>>,
    dropped: Arc<AtomicBool>,
}

impl<F: Future> Future for NotesDrop<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        self.future.as_mut().poll(context)
    }
}

impl<F> Drop for NotesDrop<F> {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Relaxed);
    }
}

fn host_failed() -> Error {
    Error::Internal {
        message: HOST_FAILED.to_owned(),
    }
}

/// Instantiate `bytes` in a fresh store of `engine`, with `register`
/// making the host registrations its imports name.
async fn instantiate(
    engine: &Engine,
    bytes: &[u8],
    register: impl FnOnce(&mut Linker<()>),
) -> (Store<()>, Instance) {
    let component = Component::new(engine, bytes)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(engine);
    register(&mut linker);
    let mut store: Store<()> = Store::new(engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// Register `answer` to answer with `future` for every call.
fn answer_with<F>(future: impl Fn() -> F + Send + Sync + 'static) -> impl FnOnce(&mut Linker<()>)
where
    F: Future<Output = Result<u32, Error>> + Send + 'static,
{
    move |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", move |_accessor: &Accessor<()>, (): ()| future())
            .expect("the registration of `answer`");
    }
}

/// Register `answer` to fail one poll after each call started.
fn answer_fails_after_a_pending_poll() -> impl FnOnce(&mut Linker<()>) {
    answer_with(|| FailsAfterAPendingPoll { polled: false })
}

/// Register `answer` to fail once `release` is released.
fn answer_fails_when(release: &Release) -> impl FnOnce(&mut Linker<()>) {
    let release = release.clone();
    answer_with(move || FailsWhenReleased(release.clone()))
}

/// Register `answer` to answer once `release` is released, and `block`
/// to block for ever.
fn answer_when(release: &Release) -> impl FnOnce(&mut Linker<()>) {
    let release = release.clone();
    move |linker| {
        answer_with(move || AnswersWhenReleased(release.clone()))(linker);
        linker
            .root()
            .func_wrap_concurrent("block", |_accessor: &Accessor<()>, (): ()| {
                core::future::pending::<Result<(), Error>>()
            })
            .expect("the registration of `block`");
    }
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the component exports `{name}`"))
}

/// Every message in an error's source chain and its debug form,
/// joined, so a cause the substrate wrapped can be matched wherever
/// it put it.
fn described(error: &Error) -> String {
    let mut out = format!("{error:?}");
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(link) = current {
        out.push_str(": ");
        out.push_str(&link.to_string());
        current = link.source();
    }
    out
}

/// Assert that `error` is the trap `expected` names.
fn assert_trap(error: &Error, expected: &str, driver: &str) {
    let described = described(error);
    assert!(
        described.contains(expected),
        "{driver} ends with the trap `{expected}`, got {described}"
    );
    assert!(
        !matches!(error, Error::Task(TaskCause::CannotEnter)),
        "{driver} reports the trap itself, not the poisoned store, got {described}"
    );
}

/// Assert that the next driver, a call of `name`, fails with the
/// cannot-enter cause, so the trap poisoned the store.
async fn assert_poisoned(store: &mut Store<()>, instance: &Instance, name: &str) {
    let error = func(instance, name)
        .call(store, &[])
        .await
        .expect_err("a poisoned store refuses the call");
    assert!(
        matches!(error, Error::Task(TaskCause::CannotEnter))
            && error.to_string().contains(CANNOT_ENTER),
        "the next driver fails with the cannot-enter cause, got {error:?}"
    );
}

/// The call of `name` that is expected to trap, and the trap.
async fn trap_of(store: &mut Store<()>, instance: &Instance, name: &str) -> Error {
    match func(instance, name).call(store, &[]).await {
        Ok(values) => panic!("`{name}` answered {values:?}, and was to end with a trap"),
        Err(error) => error,
    }
}

/// Whether each test runs with the suspend provider on, and then with
/// it off. With it on, a blocked thread suspends through the provider
/// the target has, where it has one; with it off, a block runs a
/// nested turn above the blocked call and a switch runs the thread it
/// names on the real stack. The rule is the same either way.
const PROVIDERS: [bool; 2] = [true, false];

/// An engine with the suspend provider on or off as `provider` says,
/// and with the thread built-ins allowed when `threading` says so.
fn engine(provider: bool, threading: bool) -> Engine {
    let mut config = EngineConfig::new();
    config.suspend_provider(provider);
    config.wasm_component_model_threading(threading);
    Engine::with_config(&config).expect("engine")
}

async fn it_ends_a_later_driver_when_a_callback_task_that_resolved_traps_in_a_later_turn_with(
    provider: bool,
) {
    let engine = engine(provider, false);
    let (mut store, instance) =
        instantiate(&engine, CALLBACKS, answer_fails_after_a_pending_poll()).await;

    let resolved = func(&instance, "resolve-then-trap")
        .call(&mut store, &[])
        .await
        .expect("the call answers at `task.return`, before the callback runs");
    assert_eq!(resolved.as_ref(), [Val::U32(6)]);

    // The callback the task left runs in the turn of the driver that
    // gives way to it, and its trap ends that driver.
    let error = trap_of(&mut store, &instance, "drive").await;
    assert_trap(
        &error,
        "unreachable",
        "the driver whose turn ran the callback",
    );
    assert_poisoned(&mut store, &instance, "ok").await;
}

async fn it_ends_a_later_driver_when_a_thread_that_outlived_its_tasks_host_call_traps_with(
    provider: bool,
) {
    let engine = engine(provider, true);
    let (mut store, instance) = instantiate(&engine, THREADS, |_| {}).await;

    let spawned = func(&instance, "spawn-ready")
        .call(&mut store, &[])
        .await
        .expect("the call returns before the thread it made ready runs");
    assert_eq!(spawned.as_ref(), [Val::U32(1)]);

    // The thread runs in the turn of the driver that gives way to it,
    // and its trap ends that driver.
    let error = trap_of(&mut store, &instance, "drive").await;
    assert_trap(
        &error,
        "unreachable",
        "the driver whose turn ran the thread",
    );
    assert_poisoned(&mut store, &instance, "spawn-held").await;
}

async fn it_ends_a_later_driver_when_a_thread_that_outlived_its_async_task_traps_with(
    provider: bool,
) {
    // The export is lifted `async`, so its call answers from the
    // task's channel at `task.return` rather than from the end of its
    // core function, and its task stays in the store for as long as
    // the thread it started does.
    let engine = engine(provider, true);
    let (mut store, instance) = instantiate(&engine, THREADS, |_| {}).await;

    let spawned = func(&instance, "spawn-ready-async")
        .call(&mut store, &[])
        .await
        .expect("the call answers at `task.return`, before the thread it made ready runs");
    assert_eq!(spawned.as_ref(), [Val::U32(5)]);

    // The thread runs in the turn of the driver that gives way to it,
    // and its trap ends that driver.
    let error = trap_of(&mut store, &instance, "drive").await;
    assert_trap(
        &error,
        "unreachable",
        "the driver whose turn ran the thread",
    );
    assert_poisoned(&mut store, &instance, "spawn-held").await;
}

async fn it_ends_the_switching_driver_when_a_thread_of_another_task_traps_with(provider: bool) {
    // With no provider the switch runs the thread on the real stack
    // above the built-in, and under a provider it runs on a stack of
    // its own. Either way the thread belongs to the task of the call
    // that has already returned, and its trap ends the driver that is
    // polling, which is the call that switched to it.
    let engine = engine(provider, true);
    let (mut store, instance) = instantiate(&engine, THREADS, |_| {}).await;

    let held = func(&instance, "spawn-held")
        .call(&mut store, &[])
        .await
        .expect("the call returns and leaves its thread suspended");
    assert_eq!(held.as_ref(), [Val::U32(2)]);

    let error = trap_of(&mut store, &instance, "switch-to-held").await;
    assert_trap(
        &error,
        "unreachable",
        "the call that switched to the thread",
    );
    assert_poisoned(&mut store, &instance, "spawn-held").await;
}

async fn it_ends_a_later_driver_when_a_callback_exports_host_call_fails_after_it_resolved_with(
    provider: bool,
) {
    let engine = engine(provider, false);
    let release = Release::default();
    let (mut store, instance) = instantiate(&engine, CALLBACKS, answer_fails_when(&release)).await;

    let resolved = func(&instance, "resolve-then-wait")
        .call(&mut store, &[])
        .await
        .expect("the call answers at `task.return`, before the host call fails");
    assert_eq!(resolved.as_ref(), [Val::U32(7)]);

    // The host body fails in a turn of the next driver. The call that
    // started the task has its result and never learns of the
    // failure; the driver that polled the body reports it.
    release.release();
    let error = trap_of(&mut store, &instance, "ok").await;
    assert_trap(
        &error,
        HOST_FAILED,
        "the driver whose turn polled the host body",
    );
    assert_poisoned(&mut store, &instance, "ok").await;
}

async fn it_ends_the_entry_when_a_concurrent_calls_host_call_fails_after_it_answered_with(
    provider: bool,
) {
    let engine = engine(provider, false);
    let release = Release::default();
    let (mut store, instance) = instantiate(&engine, CALLBACKS, answer_fails_when(&release)).await;
    let resolve_then_wait = func(&instance, "resolve-then-wait");
    let answered = Arc::new(Mutex::new(None));

    let entry = store
        .run_concurrent(async |accessor| {
            let resolved = resolve_then_wait.call_concurrent(accessor, &[]).await;
            *answered.lock().expect("the answer") = Some(resolved);
            // The call has answered, and the host body fails now. The
            // closure waits for ever, so only the failure ends the
            // entry.
            release.release();
            poll_fn(|_| Poll::<()>::Pending).await;
        })
        .await;

    let answered = answered
        .lock()
        .expect("the answer")
        .take()
        .expect("the call answered before the failure");
    assert_eq!(
        answered
            .expect("the call answers at `task.return`")
            .as_ref(),
        [Val::U32(7)]
    );
    let error = entry.expect_err("the failure is not lost: it ends the entry that was polling");
    assert_trap(
        &error,
        HOST_FAILED,
        "the entry whose turn polled the host body",
    );
    assert_poisoned(&mut store, &instance, "ok").await;
}

async fn it_ends_the_driver_when_a_host_call_of_a_task_no_host_call_owns_fails_with(
    provider: bool,
) {
    // `work`'s task is the callee of a call between two components.
    // No host call owns it, and its failed host call still ends the
    // driver that is polling.
    let engine = engine(provider, false);
    let (mut store, instance) =
        instantiate(&engine, GUEST_TO_GUEST, answer_fails_after_a_pending_poll()).await;

    let error = trap_of(&mut store, &instance, "run").await;
    assert_trap(
        &error,
        HOST_FAILED,
        "the driver whose turn polled the host body",
    );
    assert_poisoned(&mut store, &instance, "run").await;
}

async fn it_poisons_the_store_when_a_host_async_future_fails_with(provider: bool) {
    let engine = engine(provider, false);
    let (mut store, instance) =
        instantiate(&engine, CALLBACKS, answer_fails_after_a_pending_poll()).await;

    let error = trap_of(&mut store, &instance, "call-and-wait").await;
    assert_trap(
        &error,
        HOST_FAILED,
        "the call whose task made the host call",
    );

    // Nothing of the guest failed: the host future did, and that alone
    // poisoned the store.
    assert_poisoned(&mut store, &instance, "ok").await;
}

async fn it_ends_the_driver_of_a_synchronous_task_whose_host_call_fails_with(provider: bool) {
    // The export is lifted synchronously and blocks on its host call
    // where it stands. The call's body fails on its second poll.
    let engine = engine(provider, false);
    let (mut store, instance) = instantiate(&engine, BLOCKS, |linker| {
        answer_fails_after_a_pending_poll()(linker);
        linker
            .root()
            .func_wrap_concurrent("block", |_accessor: &Accessor<()>, (): ()| {
                core::future::pending::<Result<(), Error>>()
            })
            .expect("the registration of `block`");
    })
    .await;

    let error = trap_of(&mut store, &instance, "block-on-answer").await;
    assert_trap(
        &error,
        HOST_FAILED,
        "the call that blocked on the host call",
    );
    assert_poisoned(&mut store, &instance, "block-on-answer").await;
}

async fn it_ends_the_entry_when_a_host_call_fails_while_its_task_is_below_another_tasks_turn_with(
    provider: bool,
) {
    // Two instances of one component in one store, so that neither
    // task holds the other's instance. The first task starts a host
    // call and then blocks. With no provider its block runs a nested
    // turn, which starts the second task, which blocks in turn: the
    // body of the first task's host call fails in the second task's
    // nested turn, with the first task on the stack below it. Under a
    // provider each task suspends instead. Either way the failure ends
    // the entry that was polling.
    let engine = engine(provider, false);
    let component = Component::new(&engine, BLOCKS)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    answer_fails_after_a_pending_poll()(&mut linker);
    linker
        .root()
        .func_wrap_concurrent("block", |_accessor: &Accessor<()>, (): ()| {
            core::future::pending::<Result<(), Error>>()
        })
        .expect("the registration of `block`");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let first = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the first instance");
    let second = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the second instance");
    let call_then_block = func(&first, "call-then-block");
    let block = func(&second, "block");

    let entry = store
        .run_concurrent(async |accessor| {
            let mut calling = Box::pin(call_then_block.call_concurrent(accessor, &[]));
            let mut blocking = Box::pin(block.call_concurrent(accessor, &[]));
            poll_fn(|context| {
                let _ = calling.as_mut().poll(context);
                let _ = blocking.as_mut().poll(context);
                Poll::<()>::Pending
            })
            .await;
        })
        .await;

    let error = entry.expect_err("the failure ends the entry that was polling");
    assert_trap(
        &error,
        HOST_FAILED,
        "the entry whose turn polled the host body",
    );
    assert_poisoned(&mut store, &first, "block-on-answer").await;
}

async fn it_ends_a_later_driver_when_the_task_of_a_dropped_call_traps_with(provider: bool) {
    // `answer-then-trap` is lifted synchronously and blocks on its
    // host call where it stands. Its call is polled once, which leaves
    // the task blocked, and then dropped, which cancels nothing. Once
    // the host call answers, the task traps in the turn of a later
    // driver, and no call is left to take the trap as its own.
    let engine = engine(provider, false);
    let release = Release::default();
    let (mut store, instance) = instantiate(&engine, BLOCKS, answer_when(&release)).await;
    let answer_then_trap = func(&instance, "answer-then-trap");
    let first = {
        let mut call = Box::pin(answer_then_trap.call(&mut store, &[]));
        poll_fn(|context| Poll::Ready(call.as_mut().poll(context))).await
    };

    // With no provider the block runs a nested turn above the call,
    // which can give no control back to the host while the host call
    // is pending. The first poll therefore ends the call with the
    // stack-switch cause, and no dropped call is left behind.
    if engine.suspend_provider() == SuspendProviderKind::None {
        let Poll::Ready(Err(error)) = first else {
            panic!("with no provider the first poll ends the call, got {first:?}");
        };
        assert_trap(&error, "stack switch", "the call that blocked");
        assert_poisoned(&mut store, &instance, "ok").await;
        return;
    }
    assert!(
        first.is_pending(),
        "the call is left blocked on its host call, got {first:?}"
    );

    // A driver can answer before a turn of its runs the task, whose
    // thread resumes behind the driver's own work. The first driver
    // that fails reports the trap, not the deadlock or the
    // cannot-enter cause that a lost trap would leave.
    release.release();
    let ok = func(&instance, "ok");
    let mut answered = 0;
    let error = loop {
        match ok.call(&mut store, &[]).await {
            Ok(values) => {
                assert_eq!(values.as_ref(), [Val::U32(9)]);
                answered += 1;
                assert!(answered < 3, "a later driver meets the trap");
            }
            Err(error) => break error,
        }
    };
    assert_trap(
        &error,
        "unreachable",
        "the driver whose turn ran the dropped call's task",
    );
    assert_poisoned(&mut store, &instance, "ok").await;
}

async fn it_ends_the_entry_and_drops_the_other_call_when_one_of_two_concurrent_calls_traps_with(
    provider: bool,
) {
    let engine = engine(provider, false);
    let (mut store, instance) =
        instantiate(&engine, CALLBACKS, answer_fails_after_a_pending_poll()).await;
    let drive = func(&instance, "drive");
    let trap = func(&instance, "trap");
    let other_dropped = Arc::new(AtomicBool::new(false));
    let trapping_dropped = Arc::new(AtomicBool::new(false));

    let entry = store
        .run_concurrent(async |accessor| {
            let mut other = NotesDrop {
                future: Box::pin(drive.call_concurrent(accessor, &[])),
                dropped: other_dropped.clone(),
            };
            let mut trapping = NotesDrop {
                future: Box::pin(trap.call_concurrent(accessor, &[])),
                dropped: trapping_dropped.clone(),
            };
            let mut answers = (None, None);
            poll_fn(|context| {
                if answers.0.is_none()
                    && let Poll::Ready(answer) = Pin::new(&mut other).poll(context)
                {
                    answers.0 = Some(answer);
                }
                if answers.1.is_none()
                    && let Poll::Ready(answer) = Pin::new(&mut trapping).poll(context)
                {
                    answers.1 = Some(answer);
                }
                if answers.0.is_some() && answers.1.is_some() {
                    return Poll::Ready(());
                }
                Poll::Pending
            })
            .await;
            answers
        })
        .await;

    let error = match entry {
        Ok(answers) => panic!("the trap ends the entry, and the closure answered {answers:?}"),
        Err(error) => error,
    };
    assert_trap(&error, "unreachable", "the entry around the two calls");
    assert!(
        other_dropped.load(Ordering::Relaxed),
        "the other call's future was dropped with the closure"
    );
    assert!(
        trapping_dropped.load(Ordering::Relaxed),
        "and so was the future of the call that trapped"
    );
    assert_poisoned(&mut store, &instance, "ok").await;
}

#[wcmp_macros::test]
async fn it_ends_a_later_driver_when_a_callback_task_that_resolved_traps_in_a_later_turn() {
    for provider in PROVIDERS {
        it_ends_a_later_driver_when_a_callback_task_that_resolved_traps_in_a_later_turn_with(
            provider,
        )
        .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_a_later_driver_when_a_thread_that_outlived_its_tasks_host_call_traps() {
    for provider in PROVIDERS {
        it_ends_a_later_driver_when_a_thread_that_outlived_its_tasks_host_call_traps_with(provider)
            .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_switching_driver_when_a_thread_of_another_task_traps() {
    for provider in PROVIDERS {
        it_ends_the_switching_driver_when_a_thread_of_another_task_traps_with(provider).await;
    }
}

#[wcmp_macros::test]
async fn it_ends_a_later_driver_when_a_callback_exports_host_call_fails_after_it_resolved() {
    for provider in PROVIDERS {
        it_ends_a_later_driver_when_a_callback_exports_host_call_fails_after_it_resolved_with(
            provider,
        )
        .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_entry_when_a_concurrent_calls_host_call_fails_after_it_answered() {
    for provider in PROVIDERS {
        it_ends_the_entry_when_a_concurrent_calls_host_call_fails_after_it_answered_with(provider)
            .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_driver_when_a_host_call_of_a_task_no_host_call_owns_fails() {
    for provider in PROVIDERS {
        it_ends_the_driver_when_a_host_call_of_a_task_no_host_call_owns_fails_with(provider).await;
    }
}

#[wcmp_macros::test]
async fn it_poisons_the_store_when_a_host_async_future_fails() {
    for provider in PROVIDERS {
        it_poisons_the_store_when_a_host_async_future_fails_with(provider).await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_driver_of_a_synchronous_task_whose_host_call_fails() {
    for provider in PROVIDERS {
        it_ends_the_driver_of_a_synchronous_task_whose_host_call_fails_with(provider).await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_entry_when_a_host_call_fails_while_its_task_is_below_another_tasks_turn() {
    for provider in PROVIDERS {
        it_ends_the_entry_when_a_host_call_fails_while_its_task_is_below_another_tasks_turn_with(
            provider,
        )
        .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_the_entry_and_drops_the_other_call_when_one_of_two_concurrent_calls_traps() {
    for provider in PROVIDERS {
        it_ends_the_entry_and_drops_the_other_call_when_one_of_two_concurrent_calls_traps_with(
            provider,
        )
        .await;
    }
}

#[wcmp_macros::test]
async fn it_ends_a_later_driver_when_the_task_of_a_dropped_call_traps() {
    for provider in PROVIDERS {
        it_ends_a_later_driver_when_the_task_of_a_dropped_call_traps_with(provider).await;
    }
}

#[wcmp_macros::test]
async fn it_ends_a_later_driver_when_a_thread_that_outlived_its_async_task_traps() {
    for provider in PROVIDERS {
        it_ends_a_later_driver_when_a_thread_that_outlived_its_async_task_traps_with(provider)
            .await;
    }
}
