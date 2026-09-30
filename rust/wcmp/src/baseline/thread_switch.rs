//! Baseline tests for the five thread built-ins that suspend or
//! switch: `thread.suspend`, `thread.suspend-then-resume`,
//! `thread.yield-then-resume`, `thread.suspend-then-promote`, and
//! `thread.yield-then-promote`.
//!
//! Most exports here are synchronous. A task that must not block
//! waits on the nested turn: a suspension waits in turns run from
//! inside the built-in. With no provider, a switch it makes starts
//! the thread it names from inside the built-in, on the real stack
//! above the built-in. Under a provider the export's thread, the
//! thread of a host call, runs on a stack of its own, and a switch
//! suspends it there, as the reference suspends it. The frame that
//! started it runs the named thread, and resumes the export's thread
//! once that thread stops, if it is ready. A thread that a built-in
//! of such a task starts from inside itself, as a nested turn does,
//! suspends back into that built-in. The outcomes the tests read are
//! the same either way, except where a thread switches back to the
//! export's thread: only a provider can resume that thread then. The
//! two callback exports run on a stack of their own under a provider.
//! Each test whose outcome differs
//! says how.
//!
//! The component keeps a log in a core global: each step of an export
//! and of the threads it starts appends one digit, so the number an
//! export returns is the order in which its steps ran. The export's
//! own thread writes 1 as it begins and 3 as it goes on after the
//! built-in, and a thread it starts writes 2. A thread that must not
//! run before the built-in returns writes 4.

#![cfg(test)]

use crate::{
    Component, Engine, EngineConfig, Error, Func, Instance, Linker, Store, SuspendProviderKind, Val,
};
use wcmp_macros::component;

/// One component instance whose table holds a thread start function
/// at each entry:
///
/// - 0, `wake-main`, logs 2 and makes the export's thread ready with
///   `thread.resume-later`.
/// - 1, `log-two`, logs 2.
/// - 2, `resume-main`, logs 2 and switches back to the export's
///   thread with `thread.suspend-then-resume`.
/// - 3, `wake-main-bare` makes the export's thread ready and logs
///   nothing.
/// - 4, `wait-empty`, waits on an empty waitable set, for ever.
/// - 5, `log-four`, logs 4.
///
/// The synchronous exports are named for what they exercise. The two
/// callback exports switch to `wait-empty`, whose wait nothing can
/// end, once yielding and once suspending.
const THREADS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (table (export "__indirect_function_table") 6 funcref))
      (core instance $libc (instantiate $libc))

      (core func $task-return (canon task.return (result u32)))
      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))
      (core func $suspend (canon thread.suspend))
      (core func $suspend-then-resume (canon thread.suspend-then-resume))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $suspend-then-promote (canon thread.suspend-then-promote))
      (core func $yield-then-promote (canon thread.yield-then-promote))
      (core func $set-new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))

      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "" "thread.suspend" (func $suspend (result i32)))
        (import "" "thread.suspend-then-resume" (func $suspend-then-resume (param i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "thread.suspend-then-promote" (func $suspend-then-promote (param i32) (result i32)))
        (import "" "thread.yield-then-promote" (func $yield-then-promote (param i32) (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "libc" "__indirect_function_table" (table 6 funcref))

        (global $main (mut i32) (i32.const 0))
        (global $log (mut i32) (i32.const 0))

        (func $log (param i32)
          (global.set $log
            (i32.add (i32.mul (global.get $log) (i32.const 10)) (local.get 0))))

        (func $wake-main (param i32)
          (call $log (i32.const 2))
          (call $resume-later (global.get $main)))
        (func $log-two (param i32)
          (call $log (i32.const 2)))
        (func $resume-main (param i32)
          (call $log (i32.const 2))
          (drop (call $suspend-then-resume (global.get $main))))
        (func $wake-main-bare (param i32)
          (call $resume-later (global.get $main)))
        (func $wait-empty (param i32)
          (drop (call $wait (call $set-new) (i32.const 0))))
        (func $log-four (param i32)
          (call $log (i32.const 4)))
        (elem (table 0) (i32.const 0)
          func $wake-main $log-two $resume-main $wake-main-bare $wait-empty $log-four)

        (func $begin
          (global.set $main (call $thread-index))
          (global.set $log (i32.const 0))
          (call $log (i32.const 1)))
        (func $spawn (param $entry i32) (result i32)
          (call $new-indirect (local.get $entry) (i32.const 0)))
        (func $spawn-ready (param $entry i32) (result i32)
          (local $thread i32)
          (local.set $thread (call $spawn (local.get $entry)))
          (call $resume-later (local.get $thread))
          (local.get $thread))
        (func $end (result i32)
          (call $log (i32.const 3))
          (global.get $log))

        (func (export "suspend") (result i32)
          (call $begin)
          (drop (call $spawn-ready (i32.const 0)))
          (drop (call $suspend))
          (call $end))
        (func (export "suspend-unresumed") (result i32)
          (call $begin)
          (drop (call $suspend))
          (call $end))

        (func (export "suspend-then-resume") (result i32)
          (call $begin)
          (drop (call $suspend-then-resume (call $spawn (i32.const 0))))
          (call $end))
        (func (export "suspend-then-resume-unresumed") (result i32)
          (call $begin)
          (drop (call $suspend-then-resume (call $spawn (i32.const 1))))
          (call $end))
        (func (export "suspend-then-resume-below") (result i32)
          (call $begin)
          (drop (call $suspend-then-resume (call $spawn (i32.const 2))))
          (call $end))
        (func (export "suspend-then-resume-ready") (result i32)
          (call $begin)
          (drop (call $suspend-then-resume (call $spawn-ready (i32.const 1))))
          (call $end))

        (func (export "yield-then-resume") (result i32)
          (call $begin)
          (drop (call $yield-then-resume (call $spawn (i32.const 1))))
          (call $end))
        (func (export "yield-then-resume-waking-main") (result i32)
          (call $begin)
          (drop (call $yield-then-resume (call $spawn (i32.const 3))))
          (call $end))
        (func (export "yield-then-resume-ready") (result i32)
          (call $begin)
          (drop (call $yield-then-resume (call $spawn-ready (i32.const 1))))
          (call $end))

        (func (export "suspend-then-promote-ready") (result i32)
          (local $thread i32)
          (call $begin)
          (drop (call $spawn-ready (i32.const 5)))
          (local.set $thread (call $spawn-ready (i32.const 0)))
          (drop (call $suspend-then-promote (local.get $thread)))
          (call $end))
        (func (export "suspend-then-promote-not-ready") (result i32)
          (local $thread i32)
          (call $begin)
          (local.set $thread (call $spawn (i32.const 5)))
          (drop (call $spawn-ready (i32.const 0)))
          (drop (call $suspend-then-promote (local.get $thread)))
          (call $end))
        (func (export "suspend-then-promote-self") (result i32)
          (call $begin)
          (drop (call $suspend-then-promote (global.get $main)))
          (call $end))

        (func (export "yield-then-promote-ready") (result i32)
          (local $thread i32)
          (call $begin)
          (drop (call $spawn-ready (i32.const 5)))
          (local.set $thread (call $spawn-ready (i32.const 1)))
          (drop (call $yield-then-promote (local.get $thread)))
          (call $end))
        (func (export "yield-then-promote-not-ready") (result i32)
          (call $begin)
          (drop (call $yield-then-promote (call $spawn (i32.const 5))))
          (call $end))
        (func (export "yield-then-promote-self") (result i32)
          (call $begin)
          (drop (call $yield-then-promote (global.get $main)))
          (call $end))

        (func (export "yield-then-block") (result i32)
          (drop (call $yield-then-resume (call $spawn (i32.const 4))))
          (call $task-return (i32.const 0))
          ;; Exit.
          (i32.const 0))
        (func (export "suspend-then-block") (result i32)
          (drop (call $suspend-then-resume (call $spawn (i32.const 4))))
          (call $task-return (i32.const 0))
          ;; Exit.
          (i32.const 0))
        (func (export "never-called") (param i32 i32 i32) (result i32)
          unreachable))

      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))
          (export "thread.suspend" (func $suspend))
          (export "thread.suspend-then-resume" (func $suspend-then-resume))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "thread.suspend-then-promote" (func $suspend-then-promote))
          (export "thread.yield-then-promote" (func $yield-then-promote))
          (export "waitable-set.new" (func $set-new))
          (export "waitable-set.wait" (func $wait))))
        (with "libc" (instance $libc))))

      (func (export "suspend") (result u32) (canon lift (core func $i "suspend")))
      (func (export "suspend-unresumed") (result u32)
        (canon lift (core func $i "suspend-unresumed")))
      (func (export "suspend-then-resume") (result u32)
        (canon lift (core func $i "suspend-then-resume")))
      (func (export "suspend-then-resume-unresumed") (result u32)
        (canon lift (core func $i "suspend-then-resume-unresumed")))
      (func (export "suspend-then-resume-below") (result u32)
        (canon lift (core func $i "suspend-then-resume-below")))
      (func (export "suspend-then-resume-ready") (result u32)
        (canon lift (core func $i "suspend-then-resume-ready")))
      (func (export "yield-then-resume") (result u32)
        (canon lift (core func $i "yield-then-resume")))
      (func (export "yield-then-resume-waking-main") (result u32)
        (canon lift (core func $i "yield-then-resume-waking-main")))
      (func (export "yield-then-resume-ready") (result u32)
        (canon lift (core func $i "yield-then-resume-ready")))
      (func (export "suspend-then-promote-ready") (result u32)
        (canon lift (core func $i "suspend-then-promote-ready")))
      (func (export "suspend-then-promote-not-ready") (result u32)
        (canon lift (core func $i "suspend-then-promote-not-ready")))
      (func (export "suspend-then-promote-self") (result u32)
        (canon lift (core func $i "suspend-then-promote-self")))
      (func (export "yield-then-promote-ready") (result u32)
        (canon lift (core func $i "yield-then-promote-ready")))
      (func (export "yield-then-promote-not-ready") (result u32)
        (canon lift (core func $i "yield-then-promote-not-ready")))
      (func (export "yield-then-promote-self") (result u32)
        (canon lift (core func $i "yield-then-promote-self")))
      (func (export "yield-then-block") async (result u32)
        (canon lift (core func $i "yield-then-block") async
          (callback (core func $i "never-called"))))
      (func (export "suspend-then-block") async (result u32)
        (canon lift (core func $i "suspend-then-block") async
          (callback (core func $i "never-called")))))
    "#
);

/// A component whose `post-return`s each call one of the five
/// built-ins. The instance's may-leave flag is clear while a
/// `post-return` runs.
const POST_RETURN_CALLS: &[u8] = component!(
    r#"
    (component
      (core func $suspend (canon thread.suspend))
      (core func $suspend-then-resume (canon thread.suspend-then-resume))
      (core func $yield-then-resume (canon thread.yield-then-resume))
      (core func $suspend-then-promote (canon thread.suspend-then-promote))
      (core func $yield-then-promote (canon thread.yield-then-promote))

      (core module $m
        (import "" "thread.suspend" (func $suspend (result i32)))
        (import "" "thread.suspend-then-resume" (func $suspend-then-resume (param i32) (result i32)))
        (import "" "thread.yield-then-resume" (func $yield-then-resume (param i32) (result i32)))
        (import "" "thread.suspend-then-promote" (func $suspend-then-promote (param i32) (result i32)))
        (import "" "thread.yield-then-promote" (func $yield-then-promote (param i32) (result i32)))
        (func (export "run") (result i32) (i32.const 7))
        (func (export "suspend-after") (param i32)
          (drop (call $suspend)))
        (func (export "suspend-then-resume-after") (param i32)
          (drop (call $suspend-then-resume (i32.const 1))))
        (func (export "yield-then-resume-after") (param i32)
          (drop (call $yield-then-resume (i32.const 1))))
        (func (export "suspend-then-promote-after") (param i32)
          (drop (call $suspend-then-promote (i32.const 1))))
        (func (export "yield-then-promote-after") (param i32)
          (drop (call $yield-then-promote (i32.const 1)))))

      (core instance $m (instantiate $m
        (with "" (instance
          (export "thread.suspend" (func $suspend))
          (export "thread.suspend-then-resume" (func $suspend-then-resume))
          (export "thread.yield-then-resume" (func $yield-then-resume))
          (export "thread.suspend-then-promote" (func $suspend-then-promote))
          (export "thread.yield-then-promote" (func $yield-then-promote))))))

      (func (export "suspend") (result u32)
        (canon lift (core func $m "run") (post-return (core func $m "suspend-after"))))
      (func (export "suspend-then-resume") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "suspend-then-resume-after"))))
      (func (export "yield-then-resume") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "yield-then-resume-after"))))
      (func (export "suspend-then-promote") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "suspend-then-promote-after"))))
      (func (export "yield-then-promote") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "yield-then-promote-after")))))
    "#
);

/// Wasmtime's message for a resume of a thread that is not suspended.
const NOT_SUSPENDED: &str = "cannot resume thread which is not suspended";

/// The stack-switch cause's message.
const STACK_SWITCH: &str =
    "blocking here requires a stack switch, but this thread cannot switch its stack";

/// An engine with the thread built-ins allowed.
fn engine() -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_threading(true);
    Engine::with_backend(crate::runtime_layer::test_backend())
        .and_then(|engine| engine.with_config(&config))
        .expect("engine")
}

/// Whether the engine runs guest threads through a provider: the
/// stack-switching provider natively on x86_64 Linux, and the
/// host-suspension provider in a browser that ships JavaScript Promise
/// Integration.
fn has_provider() -> bool {
    engine().suspend_provider() != SuspendProviderKind::None
}

/// Instantiate `bytes` in a fresh store of an engine with the thread
/// built-ins allowed.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = engine();
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

/// One export of the instance, by name.
fn func(instance: &Instance, name: &str) -> Func {
    instance.get_func(name).expect("the export is declared")
}

/// Every message in an error's source chain, joined so that a trap a
/// built-in raised can be matched wherever the substrate put it.
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

/// Call `name` in a fresh instance of the thread component and expect
/// it to return the log of the steps it ran.
async fn log_of(name: &str) -> u32 {
    let (mut store, instance) = instantiate(THREADS).await;
    match func(&instance, name).call(&mut store, &[]).await {
        Ok(values) => match values.first() {
            Some(Val::U32(value)) => *value,
            other => panic!("{name} answered {other:?}"),
        },
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Call `name` in a fresh instance of `bytes` and expect it to fail,
/// reporting the message.
async fn failure_of(bytes: &[u8], name: &str) -> String {
    let (mut store, instance) = instantiate(bytes).await;
    match func(&instance, name).call(&mut store, &[]).await {
        Err(error) => chain(&error),
        Ok(values) => panic!("{name} returned {values:?} rather than trapping"),
    }
}

#[wcmp_macros::test]
async fn it_suspends_a_thread_until_a_nested_turn_runs_the_thread_that_resumes_it() {
    assert_eq!(
        log_of("suspend").await,
        123,
        "the suspension waited in a nested turn, which ran the ready \
         thread that resumed the suspended one"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_suspension_of_a_synchronous_task_that_nothing_resumes_with_the_cannot_block_cause()
 {
    let message = failure_of(THREADS, "suspend-unresumed").await;
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_starts_a_thread_that_never_ran_from_inside_a_switch_that_suspends() {
    assert_eq!(
        log_of("suspend-then-resume").await,
        123,
        "the thread switched to ran before the built-in returned, and the \
         resume it made let the suspended thread go on"
    );
}

#[wcmp_macros::test]
async fn it_keeps_a_thread_that_switched_suspended_once_the_thread_it_started_returns() {
    let message = failure_of(THREADS, "suspend-then-resume-unresumed").await;
    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "the started thread returned without resuming the suspended one, \
         which then waited for a resume nothing could make; got {message}"
    );
}

#[wcmp_macros::test]
async fn it_starts_a_thread_that_never_ran_from_inside_a_switch_that_yields() {
    assert_eq!(
        log_of("yield-then-resume").await,
        123,
        "the thread switched to ran before the built-in returned, and the \
         yielding thread went on once it had"
    );
}

#[wcmp_macros::test]
async fn it_keeps_a_thread_that_yields_to_another_ready_rather_than_suspended() {
    let message = failure_of(THREADS, "yield-then-resume-waking-main").await;
    assert!(
        message.contains(NOT_SUSPENDED),
        "the started thread's resume of the yielding thread found it ready; \
         got {message}"
    );
}

#[wcmp_macros::test]
async fn it_switches_to_a_ready_thread_a_promote_names_before_other_ready_work() {
    assert_eq!(
        log_of("suspend-then-promote-ready").await,
        123,
        "the promoted thread ran at once, ahead of the thread made ready \
         before it, and its resume let the suspended thread go on"
    );
    assert_eq!(
        log_of("yield-then-promote-ready").await,
        123,
        "the promoted thread ran at once, ahead of the thread made ready \
         before it, and the yielding thread went on after it"
    );
}

#[wcmp_macros::test]
async fn it_suspends_rather_than_switching_when_a_promote_names_a_thread_that_is_not_ready() {
    assert_eq!(
        log_of("suspend-then-promote-not-ready").await,
        123,
        "the suspension ran the ready thread, which resumed it, and the \
         thread the promote named never ran"
    );
}

#[wcmp_macros::test]
async fn it_yields_rather_than_switching_when_a_promote_names_a_thread_that_is_not_ready() {
    assert_eq!(
        log_of("yield-then-promote-not-ready").await,
        13,
        "the yield went on, and the thread the promote named never ran"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_promote_that_names_the_current_thread() {
    for name in ["suspend-then-promote-self", "yield-then-promote-self"] {
        let message = failure_of(THREADS, name).await;
        assert!(
            message.contains(NOT_SUSPENDED),
            "`{name}` trapped with Wasmtime's message; got {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_fails_a_switch_to_a_thread_that_is_not_suspended_with_wasmtimes_message() {
    for name in ["suspend-then-resume-ready", "yield-then-resume-ready"] {
        let message = failure_of(THREADS, name).await;
        assert!(
            message.contains(NOT_SUSPENDED),
            "`{name}` named a thread already made ready; got {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_switches_back_to_the_thread_below_only_where_the_started_thread_can_suspend() {
    if has_provider() {
        // The export's thread runs on a stack of its own, so its
        // switch suspends it in the provider even though its task must
        // not block. The started thread's switch back resumes it, as
        // the reference resumes it.
        assert_eq!(
            log_of("suspend-then-resume-below").await,
            123,
            "the started thread ran, switched back to the export's \
             thread, and that thread went on"
        );
        return;
    }
    let message = failure_of(THREADS, "suspend-then-resume-below").await;
    assert!(
        message.contains(STACK_SWITCH),
        "the started thread switched back to the thread that started it, \
         whose frame lies below it on the real stack; got {message}"
    );
}

#[wcmp_macros::test]
async fn it_reads_the_mark_of_a_switch_that_yielded_as_a_frame_below_that_would_go_on() {
    if has_provider() {
        // The callback export's thread runs on a stack of its own, so
        // its yield suspends it in the provider and the started thread
        // runs on a stack of its own too. That thread suspends in its
        // wait, the export's thread resumes in a later turn and
        // returns, and the waiting thread leaves with the task.
        let (mut store, instance) = instantiate(THREADS).await;
        let result = func(&instance, "yield-then-block")
            .call(&mut store, &[])
            .await
            .expect("the yielding thread goes on while the started thread waits");
        assert_eq!(result.as_ref(), [Val::U32(0)]);
        return;
    }
    let message = failure_of(THREADS, "yield-then-block").await;
    assert!(
        message.contains(STACK_SWITCH),
        "the started thread blocked above a thread that yielded to it, \
         which a stack switch would let go on; got {message}"
    );
}

#[wcmp_macros::test]
async fn it_reads_the_mark_of_a_switch_that_suspended_as_a_frame_below_that_would_not_go_on() {
    let message = failure_of(THREADS, "suspend-then-block").await;
    assert!(
        message.contains("deadlock detected: event loop cannot make further progress"),
        "the started thread blocked above a thread that stays suspended, \
         so nothing below it could go on; got {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_each_suspending_built_in_when_the_instance_may_not_be_left() {
    for name in [
        "suspend",
        "suspend-then-resume",
        "yield-then-resume",
        "suspend-then-promote",
        "yield-then-promote",
    ] {
        let message = failure_of(POST_RETURN_CALLS, name).await;
        assert!(
            message.contains("cannot leave component instance"),
            "the built-in `{name}`'s post-return calls was refused with the \
             cannot-leave cause, got {message}"
        );
    }
}
