//! Baseline tests for the calls the polyfill itself makes into a
//! guest: a `cabi_realloc` and an export's `post-return`.
//!
//! A realloc runs as a task with one fresh thread. Its context slots
//! start at zero and end with it, so a slot the realloc sets reaches
//! neither the export that runs next nor the task that made the host
//! call whose result is being lowered. The instance may not be left
//! while either call runs, which a host function the guest calls from
//! inside the call reads off the store's records.
//!
//! Reallocs nest: a realloc that calls the host has the host's result
//! lowered back into the guest, and that lowering asks for memory
//! again. Each level is its own task with its own thread, and the
//! slots and the flag the outer level was running with come back as
//! the inner level ends.
//!
//! Every `cabi_realloc` below is the same bump allocator: it rounds
//! the bump pointer up to the alignment it is asked for, hands back
//! that address, and advances the pointer by the size it is asked
//! for. What each one does before it allocates is what its test is
//! about.

#![cfg(test)]

use std::sync::{Arc, Mutex};

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

/// What a host function saw of the store's records while it ran.
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
    /// Whether each component instance of the store could be left.
    may_leave: Vec<bool>,
    /// The context slots of the current thread, and `None` when no
    /// thread is running.
    context: Option<[i32; 2]>,
}

/// A component whose `cabi_realloc` calls a host function, so the
/// host can read the store's records with the realloc's own task on
/// the stack. The export takes a `string`, so the argument lowering
/// calls the realloc.
const REALLOC_CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $cset (canon context.set i32 0))
      (core module $libc
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ptr i32)
          (call $cset (i32.const 100))
          (drop (call $probe (i32.const 1)))
          (global.set $bump
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (local.set $ptr (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (local.get 3)))
          (local.get $ptr)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "probe" (func $probe'))
        (export "context.set" (func $cset))))))
      (core module $M
        (func (export "run") (param i32 i32) (result i32)
          (i32.const 0)))
      (core instance $m (instantiate $M))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc")))))
    "#
);

/// The same component, with a realloc that traps once the host has
/// read the records.
const REALLOC_TRAPS: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core func $cset (canon context.set i32 0))
      (core module $libc
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (call $cset (i32.const 100))
          (drop (call $probe (i32.const 1)))
          (unreachable)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "probe" (func $probe'))
        (export "context.set" (func $cset))))))
      (core module $M
        (func (export "run") (param i32 i32) (result i32)
          (i32.const 0)))
      (core instance $m (instantiate $M))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc")))))
    "#
);

/// A component whose `post-return` calls a host function, so the host
/// can read the store's records while the post-return runs.
const POST_RETURN_CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (core func $probe' (canon lower (func $probe)))
      (core module $M
        (import "" "probe" (func $probe (param i32) (result i32)))
        (func (export "run") (result i32)
          (i32.const 5))
        (func (export "post-return") (param i32)
          (drop (call $probe (i32.const 1)))))
      (core instance $m (instantiate $M (with "" (instance
        (export "probe" (func $probe'))))))
      (func (export "run") (result u32)
        (canon lift (core func $m "run")
          (post-return (core func $m "post-return")))))
    "#
);

/// Instantiate `bytes` with two host functions, and call the `run`
/// export with `arguments`. `probe` records the store's records each
/// time the guest calls it; `make` hands the guest a `string`, whose
/// lowering asks the guest's `cabi_realloc` for memory. Answers what
/// the call produced, what `probe` saw on each of its calls in the
/// order they ran, and what the store held after the call.
async fn run_with_probe(
    bytes: &[u8],
    arguments: Vec<Val>,
) -> (Result<Box<[Val]>>, Vec<Seen>, Seen) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let tables = store.tables_handle();
    let during: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = during.clone();

    let mut linker: Linker<()> = Linker::new(&engine);
    linker.root().func_wrap(
        "probe",
        move |_: HostCall<'_, ()>, (x,): (u32,)| -> Result<u32> {
            let guard = tables.lock().expect("handle tables");
            recorded.lock().expect("record").push(Seen {
                scopes: guard.tasks.scopes().len(),
                tasks: guard.tasks.task_count(),
                subtasks: guard.tasks.subtask_count(),
                threads: guard.tasks.thread_count(),
                may_leave: guard
                    .tasks
                    .instances()
                    .iter()
                    .map(|record| record.may_leave)
                    .collect(),
                context: guard
                    .tasks
                    .current_thread()
                    .and_then(|thread| guard.tasks.thread(thread))
                    .map(|record| record.context),
            });
            Ok(x)
        },
    );
    linker.root().func_wrap(
        "make",
        |_: HostCall<'_, ()>, (_,): (u32,)| -> Result<String> { Ok("hi".to_owned()) },
    );

    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let run = instance.get_func("run").expect("run export");
    let result = run.call(&mut store, &arguments).await;

    let guard = store.tables().lock().expect("handle tables");
    let after = Seen {
        scopes: guard.tasks.scopes().len(),
        tasks: guard.tasks.task_count(),
        subtasks: guard.tasks.subtask_count(),
        threads: guard.tasks.thread_count(),
        may_leave: guard
            .tasks
            .instances()
            .iter()
            .map(|record| record.may_leave)
            .collect(),
        context: guard
            .tasks
            .current_thread()
            .and_then(|thread| guard.tasks.thread(thread))
            .map(|record| record.context),
    };
    drop(guard);
    let during = during.lock().expect("record").clone();
    (result, during, after)
}

#[wcmp_macros::test]
async fn it_runs_a_realloc_on_a_task_of_its_own_that_may_not_leave() {
    let (result, during, after) =
        run_with_probe(REALLOC_CALLS_THE_HOST, vec![Val::String("hi".into())]).await;
    assert!(result.is_ok(), "the call returned");
    assert_eq!(
        during,
        vec![Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            may_leave: vec![false],
            context: Some([100, 0]),
        }],
        "the realloc's task sits on the export's task, with the subtask of the \
         host call on top of both; the realloc's own fresh thread carries the \
         slot it set, and its instance may not be left"
    );
    assert_eq!(
        after,
        Seen {
            may_leave: vec![true],
            ..Seen::default()
        },
        "the realloc's task and its thread are gone and the instance may be \
         left again"
    );
}

#[wcmp_macros::test]
async fn it_ends_the_realloc_task_when_the_realloc_traps() {
    let (result, during, after) =
        run_with_probe(REALLOC_TRAPS, vec![Val::String("hi".into())]).await;
    assert!(
        result.is_err(),
        "the trapping realloc failed the call that asked for memory"
    );
    assert_eq!(
        during,
        vec![Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            may_leave: vec![false],
            context: Some([100, 0]),
        }],
        "the realloc had its own task and thread before it trapped"
    );
    assert_eq!(
        after,
        Seen {
            may_leave: vec![true],
            ..Seen::default()
        },
        "the trap left no task, no thread, and no cleared flag behind"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_post_return_with_the_instance_flagged_as_one_that_may_not_leave() {
    let (result, during, after) = run_with_probe(POST_RETURN_CALLS_THE_HOST, Vec::new()).await;
    assert_eq!(
        result.expect("the call returned").first().cloned(),
        Some(Val::U32(5)),
        "the caller observed the result before the post-return ran"
    );
    assert_eq!(
        during,
        vec![Seen {
            scopes: 2,
            tasks: 1,
            subtasks: 1,
            threads: 1,
            may_leave: vec![false],
            context: Some([0, 0]),
        }],
        "the post-return runs inside the export's own task, and the instance \
         may not be left while it does"
    );
    assert_eq!(
        after,
        Seen {
            may_leave: vec![true],
            ..Seen::default()
        },
        "the instance may be left again once the post-return has returned"
    );
}

/// A component with two `cabi_realloc`s of the same component
/// instance, one nested inside the other. The export takes a
/// `string`, so the argument lowering calls the outer one; that
/// realloc calls a host function returning a `string`, and the
/// lowering of that result calls the inner one. Each realloc writes
/// its own context slot and calls the probe, and the outer one reads
/// its slot back once the inner one has returned, trapping if the
/// nested call left anything of its own behind.
///
/// The two are separate core functions because a single one cannot
/// be written: the module that defines a realloc cannot import the
/// lowered host function whose own options name that same realloc.
const NESTED_REALLOC: &[u8] = component!(
    r#"
    (component
      (import "probe" (func $probe (param "x" u32) (result u32)))
      (import "make" (func $make (param "x" u32) (result string)))
      (core func $probe' (canon lower (func $probe)))
      (core func $cget (canon context.get i32 0))
      (core func $cset (canon context.set i32 0))
      (core module $inner
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ptr i32)
          (if (i32.ne (call $cget) (i32.const 0)) (then (unreachable)))
          (call $cset (i32.const 200))
          (drop (call $probe (i32.const 1)))
          (global.set $bump
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (local.set $ptr (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (local.get 3)))
          (local.get $ptr)))
      (core instance $i (instantiate $inner (with "" (instance
        (export "probe" (func $probe'))
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (core func $make' (canon lower (func $make)
        (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
      (core module $libc
        (import "" "make" (func $make (param i32 i32)))
        (import "" "probe" (func $probe (param i32) (result i32)))
        (import "" "context.get" (func $cget (result i32)))
        (import "" "context.set" (func $cset (param i32)))
        (memory (export "memory") 1)
        (global $bump (mut i32) (i32.const 16))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (local $ptr i32)
          (if (i32.ne (call $cget) (i32.const 0)) (then (unreachable)))
          (call $cset (i32.const 100))
          (call $make (i32.const 1) (i32.const 8))
          (if (i32.ne (call $cget) (i32.const 100)) (then (unreachable)))
          (drop (call $probe (i32.const 2)))
          (global.set $bump
            (i32.and
              (i32.add (global.get $bump) (i32.sub (local.get 2) (i32.const 1)))
              (i32.sub (i32.const 0) (local.get 2))))
          (local.set $ptr (global.get $bump))
          (global.set $bump (i32.add (global.get $bump) (local.get 3)))
          (local.get $ptr)))
      (core instance $c (instantiate $libc (with "" (instance
        (export "make" (func $make'))
        (export "probe" (func $probe'))
        (export "context.get" (func $cget))
        (export "context.set" (func $cset))))))
      (core module $M
        (func (export "run") (param i32 i32) (result i32)
          (i32.const 0)))
      (core instance $m (instantiate $M))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc")))))
    "#
);

#[wcmp_macros::test]
async fn it_runs_a_nested_realloc_on_a_task_and_thread_of_its_own() {
    let (result, during, after) =
        run_with_probe(NESTED_REALLOC, vec![Val::String("hi".into())]).await;
    assert!(
        result.is_ok(),
        "the outer realloc read its own slot back, so it never trapped"
    );
    let [inner, outer] = during.as_slice() else {
        panic!("both reallocs called the probe, innermost first: {during:?}");
    };
    assert_eq!(
        inner,
        &Seen {
            scopes: 4,
            tasks: 3,
            subtasks: 1,
            threads: 3,
            may_leave: vec![false],
            context: Some([200, 0]),
        },
        "the nested realloc's task sits on the outer realloc's task, which \
         sits on the export's task; its thread is a third one that started at \
         zero and carries the slot it set, and the instance still may not be \
         left"
    );
    assert_eq!(
        outer,
        &Seen {
            scopes: 3,
            tasks: 2,
            subtasks: 1,
            threads: 2,
            may_leave: vec![false],
            context: Some([100, 0]),
        },
        "the nested realloc's task and thread ended with it, the outer \
         realloc reads its own slot again, and the flag the nested call gave \
         back is still the cleared one the outer call is owed"
    );
    assert_eq!(
        after,
        Seen {
            may_leave: vec![true],
            ..Seen::default()
        },
        "neither level left a task, a thread, or a cleared flag behind"
    );
}
