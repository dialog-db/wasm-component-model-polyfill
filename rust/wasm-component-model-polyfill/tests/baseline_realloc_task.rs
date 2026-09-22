//! Baseline tests for the calls the polyfill itself makes into a
//! guest: a `cabi_realloc` and an export's `post-return`.
//!
//! A realloc runs as a task with one fresh thread. Its context slots
//! start at zero and end with it, so a slot the realloc sets reaches
//! neither the export that runs next nor the task that made the host
//! call whose result is being lowered. A post-return runs inside the
//! export's own task, and a slot it sets is that task's thread's.
//!
//! The instance may not be left while either call runs, so neither
//! can reach a host function, a resource built-in, or another
//! component: that is what `baseline_may_leave` proves, and it is
//! why nothing here reads the store's records from inside one of the
//! two calls. It also means a realloc cannot nest inside another,
//! because the only way to reach a second one is a host call the
//! first would have to make.
//!
//! The instance may be left again once the call the polyfill made is
//! over, however it ended, and the two tests that read the store's
//! records say so by reaching a host import from an ordinary export
//! afterwards — after a realloc that trapped as well as after a
//! post-return that returned.
//!
//! Every `cabi_realloc` below is the same bump allocator: it rounds
//! the bump pointer up to the alignment it is asked for, hands back
//! that address, and advances the pointer by the size it is asked
//! for. What each one does before it allocates is what its test is
//! about.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, HostCall, Linker, Result, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component whose `cabi_realloc` reads its context slot, writes
/// it, and reads it back, and whose export returns its own slot. The
/// export takes a `string`, so the host's argument lowering calls the
/// realloc before the export runs.
const REALLOC_DURING_ARGUMENT_LOWERING: &[u8] = component!(
    r#"
    (component
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $libc
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ptr i32)
          (if (i32.ne (call $cget) (i32.const 0)) (then (unreachable)))
          (call $cset (i32.const 100))
          (if (i32.ne (call $cget) (i32.const 100)) (then (unreachable)))
          (global.set $bump
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (local.set $ptr (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (local.get 3)))
          (local.get $ptr)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (core module $M
        (import "" "context.get" (func $cget (result i32)))
        (func (export "run") (param i32 i32) (result i32)
          (call $cget)))
      (core instance $m (instantiate $M (with "" (instance
        (export "context.get" (func $cget))))))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc")))))
    "#
);

#[wcmp_macros::test]
async fn it_keeps_a_slot_a_realloc_set_away_from_the_export() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, REALLOC_DURING_ARGUMENT_LOWERING)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");

    let result = run
        .call(&mut store, &[Val::String("hi".into())])
        .await
        .expect("the realloc read and wrote its own slot and the call returned");

    assert_eq!(
        result.first().cloned(),
        Some(Val::U32(0)),
        "the realloc found zero in slot 0 and set it to 100 on its own thread, \
         and the export's thread still reads zero"
    );
}

/// A component whose export sets its context slot, calls a host
/// function that returns a `string`, and reads its slot back. The
/// lowering of the host's result calls the realloc, which writes a
/// slot of its own.
const REALLOC_DURING_RESULT_LOWERING: &[u8] = component!(
    r#"
    (component
      (import "make" (func $make (result string)))
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $libc
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ptr i32)
          (if (i32.ne (call $cget) (i32.const 0)) (then (unreachable)))
          (call $cset (i32.const 100))
          (if (i32.ne (call $cget) (i32.const 100)) (then (unreachable)))
          (global.set $bump
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (local.set $ptr (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (local.get 3)))
          (local.get $ptr)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (core func $make' (canon lower (func $make)
        (memory (core memory $c "memory"))
        (realloc (core func $c "realloc"))))
      (core module $M
        (import "" "make" (func $make (param i32)))
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (func (export "run") (result i32)
          (call $cset (i32.const 7))
          (call $make (i32.const 8))
          (call $cget)))
      (core instance $m (instantiate $M (with "" (instance
        (export "make" (func $make'))
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run"))))
    "#
);

#[wcmp_macros::test]
async fn it_keeps_a_slot_a_realloc_set_away_from_the_task_that_called_the_host() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, REALLOC_DURING_RESULT_LOWERING)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("make", |_: HostCall<'_, ()>, (): ()| -> Result<String> {
            Ok("hi".to_owned())
        });
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");

    let result = run
        .call(&mut store, &[])
        .await
        .expect("the host's string crossed into the guest");

    assert_eq!(
        result.first().cloned(),
        Some(Val::U32(7)),
        "the realloc that lowered the host's result saw zero and set 100 on \
         its own thread, and the task that made the call still reads its own 7"
    );
}

/// What the store held after a call.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Seen {
    /// How deep the stack of current scopes was.
    scopes: usize,
    /// How many task records the store held.
    tasks: usize,
    /// How many subtask records the store held.
    subtasks: usize,
    /// How many thread records the store held.
    threads: usize,
}

/// A component whose `cabi_realloc` writes a context slot and then
/// traps. The export takes a `string`, so the argument lowering
/// calls the realloc. The `check` export beside it calls the `ping`
/// host import, which the guest can only reach while the instance
/// may be left.
const REALLOC_TRAPS: &[u8] = component!(
    r#"
    (component
      (import "ping" (func $ping (result u32)))
      (core func $cset (canon context.set i32 0))
      (core func $ping' (canon lower (func $ping)))
      (core module $libc
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (call $cset (i32.const 100))
          (unreachable)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "context.set" (func $cset))))))
      (core module $M
        (import "" "ping" (func $ping (result i32)))
        (func (export "run") (param i32 i32) (result i32)
          (i32.const 0))
        (func (export "check") (result i32)
          (call $ping)))
      (core instance $m (instantiate $M (with "" (instance
        (export "ping" (func $ping'))))))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc"))))
      (func (export "check") (result u32)
        (canon lift (core func $m "check"))))
    "#
);

/// A component whose `post-return` writes a context slot. The export
/// returns a `u32`, so nothing of the call needs the guest's memory.
/// The `check` export beside it calls the `ping` host import, which
/// the guest can only reach while the instance may be left.
const POST_RETURN_WRITES_A_SLOT: &[u8] = component!(
    r#"
    (component
      (import "ping" (func $ping (result u32)))
      (core func $cset (canon context.set i32 0))
      (core func $ping' (canon lower (func $ping)))
      (core module $M
        (import "" "context.set" (func $cset (param i32)))
        (import "" "ping" (func $ping (result i32)))
        (func (export "run") (result i32)
          (i32.const 5))
        (func (export "post-return") (param i32)
          (call $cset (i32.const 100)))
        (func (export "check") (result i32)
          (call $ping)))
      (core instance $m (instantiate $M (with "" (instance
        (export "context.set" (func $cset))
        (export "ping" (func $ping'))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "post-return"))))
      (func (export "check") (result u32)
        (canon lift (core func $m "check"))))
    "#
);

/// What the `ping` host import answers, so that a `check` call that
/// reached the host is told apart from one that answered on its own.
const PING: u32 = 9;

/// Instantiate `bytes`, call the `run` export with `arguments`,
/// answer what that call produced and what the store held once it
/// had ended, and then call the `check` export and answer that too.
///
/// Nothing reads the store while the `run` call is in flight: a host
/// function is the only thing that could, and a realloc and a
/// post-return may not call one, because the instance may not be
/// left while either runs. That refusal is what
/// `baseline_may_leave` proves. The `check` call afterwards is the
/// other half of the same property: `check` is an ordinary export,
/// and the `ping` import it calls leaves the instance, so it
/// returns only if the flag the polyfill cleared around its own
/// call has come back.
async fn run_and_read_records(
    bytes: &[u8],
    arguments: Vec<Val>,
) -> (Result<Box<[Val]>>, Seen, Result<Box<[Val]>>) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap("ping", |_: HostCall<'_, ()>, (): ()| -> Result<u32> {
            Ok(PING)
        });
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let result = run.call(&mut store, &arguments).await;

    // The guard is scoped so that it is gone before the `check`
    // call below, which takes the same lock as it runs.
    let after = {
        let guard = store.tables().lock().expect("handle tables");
        Seen {
            scopes: guard.tasks.scopes().len(),
            tasks: guard.tasks.task_count(),
            subtasks: guard.tasks.subtask_count(),
            threads: guard.tasks.thread_count(),
        }
    };

    let check = instance.get_func("check").expect("check export");
    let reached = check.call(&mut store, &[]).await;
    (result, after, reached)
}

#[wcmp_macros::test]
async fn it_ends_the_realloc_task_when_the_realloc_traps() {
    let (result, after, reached) =
        run_and_read_records(REALLOC_TRAPS, vec![Val::String("hi".into())]).await;
    assert!(
        result.is_err(),
        "the trapping realloc failed the call that asked for memory"
    );
    assert_eq!(
        after,
        Seen::default(),
        "the trap left no task, no thread, and no scope behind"
    );
    assert_eq!(
        reached.expect("the check export returned").first().cloned(),
        Some(Val::U32(PING)),
        "the trap gave back the may-leave flag the realloc call had cleared, \
         so an ordinary export of the same instance reaches the host again"
    );
}

#[wcmp_macros::test]
async fn it_ends_the_export_task_when_the_post_return_has_run() {
    let (result, after, reached) =
        run_and_read_records(POST_RETURN_WRITES_A_SLOT, Vec::new()).await;
    assert_eq!(
        result.expect("the call returned").first().cloned(),
        Some(Val::U32(5)),
        "the caller observed the result before the post-return ran"
    );
    assert_eq!(
        after,
        Seen::default(),
        "the export's task and its thread are gone once the post-return has run"
    );
    assert_eq!(
        reached.expect("the check export returned").first().cloned(),
        Some(Val::U32(PING)),
        "the end of the export's task gave back the may-leave flag the \
         post-return had cleared, so an ordinary export of the same instance \
         reaches the host again"
    );
}
