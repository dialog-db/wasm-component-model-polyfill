//! Baseline tests for the two cancellation built-ins, `task.cancel`
//! and `subtask.cancel`.
//!
//! Cancellation is not built, and the cancelled event is never
//! delivered. The built-ins are accepted all the same, because the
//! binding layer of the Rust toolchain links `task.cancel` in every
//! `async` export and `subtask.cancel` in every awaited import. A
//! component that imports either one instantiates and runs every
//! path that does not cancel, and a call to either fails with
//! `Error::Unsupported`.
//!
//! The may-leave check comes first. A call while the instance's
//! may-leave flag is clear, as it is during a `post-return`, fails
//! with the cannot-leave cause and never reaches the unsupported
//! failure.

#![cfg(test)]

use wasm_component_model_polyfill::{
    Component, Engine, Error, Instance, InstantiationError, Linker, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Which built-in the `post-return` of `run` calls, chosen by the
/// `select` export before the call. Zero calls neither.
const NEITHER: u32 = 0;
/// Call `task.cancel`.
const TASK_CANCEL: u32 = 1;
/// Call `subtask.cancel`.
const SUBTASK_CANCEL: u32 = 2;

/// A component that imports both cancellation built-ins.
///
/// `answer` never cancels. `cancel-task` and `cancel-subtask` each
/// call one built-in. The `post-return` of `run` calls the built-in
/// `select` chose, with the instance's may-leave flag clear.
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
        (func (export "cancel-task") (call $task-cancel))
        (func (export "cancel-subtask") (result i32) (call $subtask-cancel (i32.const 1)))
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
      (func (export "cancel-task") (canon lift (core func $i "cancel-task")))
      (func (export "cancel-subtask") (result u32) (canon lift (core func $i "cancel-subtask")))
      (func (export "select") (param "w" u32) (canon lift (core func $i "select")))
      (func (export "run") (result u32)
        (canon lift (core func $i "run")
          (post-return (core func $i "post-return")))))
    "#
);

async fn instantiate() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, CANCELS)
        .await
        .expect("both cancellation built-ins are accepted at translation");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("a component that links both built-ins instantiates");
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

/// The structured error a built-in failed with, read back out of the
/// substrate failure the guest call reports.
fn built_in_error(error: &Error) -> Option<&Error> {
    match error {
        Error::Instantiation(inner) => match inner.as_ref() {
            InstantiationError::SubstrateFailure(cause) => cause.downcast_ref::<Error>(),
            _ => None,
        },
        _ => None,
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

fn assert_unsupported(error: &Error, built_in: &str) {
    assert!(
        matches!(
            built_in_error(error),
            Some(Error::Unsupported { feature }) if feature.contains(built_in)
        ),
        "a call to `{built_in}` must fail with Error::Unsupported naming it, got {error:?}"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_path_that_does_not_cancel_in_a_component_that_links_both_built_ins() {
    let (mut store, instance) = instantiate().await;

    let results = call(&mut store, &instance, "answer", &[]).await;

    assert!(
        matches!(results.as_slice(), [Val::U32(42)]),
        "the export that never cancels returns its result: {results:?}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_call_to_task_cancel_with_the_unsupported_error() {
    let (mut store, instance) = instantiate().await;

    let error = call_failing(&mut store, &instance, "cancel-task", &[]).await;

    assert_unsupported(&error, "task.cancel");
}

#[wcmp_macros::test]
async fn it_fails_a_call_to_subtask_cancel_with_the_unsupported_error() {
    let (mut store, instance) = instantiate().await;

    let error = call_failing(&mut store, &instance, "cancel-subtask", &[]).await;

    assert_unsupported(&error, "subtask.cancel");
}

#[wcmp_macros::test]
async fn it_checks_the_may_leave_flag_before_failing_either_built_in() {
    for which in [TASK_CANCEL, SUBTASK_CANCEL] {
        let (mut store, instance) = instantiate().await;
        call(&mut store, &instance, "select", &[Val::U32(which)]).await;

        // The export's result is lifted first, and the `post-return`
        // then runs with the instance's may-leave flag clear.
        let error = call_failing(&mut store, &instance, "run", &[]).await;

        let message = chain(&error);
        assert!(
            message.contains("cannot leave component instance"),
            "a cancellation built-in called from a post-return must fail with \
             the cannot-leave cause: {message}"
        );
        assert!(
            !matches!(built_in_error(&error), Some(Error::Unsupported { .. })),
            "the may-leave check comes before the unsupported failure: {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_runs_a_post_return_that_calls_neither_built_in() {
    let (mut store, instance) = instantiate().await;
    call(&mut store, &instance, "select", &[Val::U32(NEITHER)]).await;

    let results = call(&mut store, &instance, "run", &[]).await;

    assert!(
        matches!(results.as_slice(), [Val::U32(5)]),
        "the export returns once its post-return has run: {results:?}"
    );
}
