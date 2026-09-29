//! Baseline tests for the `task.return` built-in on components
//! written by hand.
//!
//! A `canon task.return` definition translates and instantiates: the
//! guest imports it as a core function whose parameters are the
//! flattened result. What the built-in then does depends on the task
//! on top of the store's stack of current scopes, and two of its
//! refusals are reachable from a synchronous component.
//!
//! A synchronous export returns its result by returning from its
//! core function, so a `task.return` inside one is refused. A
//! `cabi_realloc` runs as a task of its own whose instance may not be
//! left, so a `task.return` inside one is refused for that reason
//! first. Every other rule of the built-in needs a task whose lift is
//! `async`, which no host call reaches yet; the crate's own tests
//! prove those against tasks created through the store's records.

#![cfg(test)]

use core::fmt::Write as _;

use wcmp::{Component, Engine, Error, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that defines `task.return` and hands it to a core
/// module, and does nothing else with it. This is the shape the
/// conformance corpus instantiates to check that the definition
/// translates at all.
const DEFINES_TASK_RETURN: &[u8] = component!(
    r#"
    (component
      (core module $m
        (import "" "task.return" (func $task-return (param i32))))
      (core func $task-return (canon task.return (result u32)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return)))))))
    "#
);

/// A component whose synchronous export calls `task.return` before it
/// returns its own result.
const SYNCHRONOUS_EXPORT_RETURNS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run") (result i32)
          (call $task-return (i32.const 7))
          (i32.const 9)))
      (core instance $i (instantiate $m
        (with "" (instance (export "task.return" (func $task-return))))))
      (func (export "run") (result u32)
        (canon lift (core func $i "run"))))
    "#
);

/// A component whose `cabi_realloc` calls `task.return`. The export
/// takes a `string`, so the host's argument lowering calls the
/// realloc before the export runs.
const REALLOC_RETURNS: &[u8] = component!(
    r#"
    (component
      (core func $task-return (canon task.return (result u32)))
      (core module $libc
        (import "" "task.return" (func $task-return (param i32)))
        (memory (export "memory") 1)
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (call $task-return (i32.const 7))
          (i32.const 16)))
      (core instance $c (instantiate $libc
        (with "" (instance (export "task.return" (func $task-return))))))
      (core module $M
        (func (export "run") (param i32 i32) (result i32)
          (i32.const 9)))
      (core instance $m (instantiate $M))
      (func (export "run") (param "x" string) (result u32)
        (canon lift (core func $m "run")
          (memory (core memory $c "memory"))
          (realloc (core func $c "realloc")))))
    "#
);

/// Every message in an error's source chain, joined so that a trap
/// message anywhere in the chain can be matched.
fn chain(err: &Error) -> String {
    let mut out = String::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(err) = current {
        if !out.is_empty() {
            out.push_str(": ");
        }
        let _ = write!(out, "{err}");
        current = err.source();
    }
    out
}

/// Instantiate `bytes` and call its `run` export with `arguments`.
async fn run(bytes: &[u8], arguments: Vec<Val>) -> Result<Box<[Val]>, Error> {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let linker: Linker<()> = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the instantiation builds the built-in");
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &arguments).await
}

#[wcmp_macros::test]
async fn it_translates_and_instantiates_a_task_return_definition() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, DEFINES_TASK_RETURN)
        .await
        .expect("the `task.return` definition translates");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    linker
        .instantiate(&mut store, &component)
        .await
        .expect("the built-in satisfies the core module's import");
}

#[wcmp_macros::test]
async fn it_refuses_a_return_from_a_synchronous_export() {
    // The task of a synchronous export was lifted without the
    // `async` option, and such a task returns its result by
    // returning from its core function.
    let err = run(SYNCHRONOUS_EXPORT_RETURNS, Vec::new())
        .await
        .expect_err("the synchronous export's return is refused");

    let text = chain(&err);
    assert!(
        text.contains("`task.return` called for a task that was not lifted `async`"),
        "expected the return-from-synchronous-task cause, got {text}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_return_from_a_realloc() {
    // A realloc runs as a task of its own, and the instance may not
    // be left while it runs, so the cannot-leave cause is the one
    // the built-in reports — ahead of the fact that the realloc's
    // task was not lifted `async` either.
    let err = run(REALLOC_RETURNS, vec![Val::String("hi".into())])
        .await
        .expect_err("the realloc's return is refused");

    let text = chain(&err);
    assert!(
        text.contains("cannot leave component instance"),
        "expected the cannot-leave cause, got {text}"
    );
}
