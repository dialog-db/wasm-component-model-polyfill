//! Baseline tests for `thread.index`, `thread.new-indirect`, and
//! `thread.resume-later`.
//!
//! The components here keep what their threads observe in core
//! globals, and export a synchronous reader for each, so a test calls
//! the export under test and then reads back what happened. The
//! start functions sit in a table a separate core instance exports,
//! the shape a C toolchain gives its `__indirect_function_table`.
//!
//! A task's implicit thread takes the first free index of its
//! instance's thread table as the task starts, and a thread the
//! built-in creates takes the next one, so the indices the tests
//! expect are the ones Wasmtime hands out for the same calls.
//!
//! The `i64` start function of a 64-bit memory cannot be declared in
//! a component today: the validator the translator runs refuses any
//! start function type other than `(i32) -> ()`. Its test builds the
//! `i64` form of the built-ins over a real instance's table instead,
//! and runs them as the guest's call would.

#![cfg(test)]

use crate::{Component, Engine, EngineConfig, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// One component instance whose table holds a start function at 0,
/// nothing at 1, and a function of another type at 2.
///
/// - `spawn` is a callback export. Its core function reads its own
///   index, creates a thread at table entry 0 with the context 42,
///   makes it ready, notes whether the thread has run yet, and
///   yields. The start function records its context and the index
///   `thread.index` answers it. The callback returns the thread's
///   index as the task's result.
/// - `new-empty` and `new-wrong-type` create a thread at entries 1
///   and 2.
/// - `resume-self` resumes the current thread, and `resume-twice`
///   creates a thread and resumes it twice.
/// - `resume-unknown` resumes an index no thread holds.
const THREADS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (table (export "__indirect_function_table") 3 funcref))
      (core instance $libc (instantiate $libc))

      (core func $task-return (canon task.return (result u32)))
      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))

      (core module $m
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "libc" "__indirect_function_table" (table 3 funcref))

        (global $implicit-index (mut i32) (i32.const 0))
        (global $created-index (mut i32) (i32.const 0))
        (global $ran-before-yield (mut i32) (i32.const -1))
        (global $context (mut i32) (i32.const 0))
        (global $seen-index (mut i32) (i32.const 0))

        (func $start (param i32)
          (global.set $context (local.get 0))
          (global.set $seen-index (call $thread-index)))
        (func $wrong-type (param i32) (result i32) (local.get 0))
        (elem (table 0) (i32.const 0) func $start)
        (elem (table 0) (i32.const 2) func $wrong-type)

        (func (export "spawn") (result i32)
          (global.set $implicit-index (call $thread-index))
          (global.set $created-index
            (call $new-indirect (i32.const 0) (i32.const 42)))
          (call $resume-later (global.get $created-index))
          (global.set $ran-before-yield (global.get $seen-index))
          ;; Yield.
          (i32.const 1))
        (func (export "spawn-callback") (param i32 i32 i32) (result i32)
          (call $task-return (global.get $seen-index))
          ;; Exit.
          (i32.const 0))

        (func (export "new-empty")
          (drop (call $new-indirect (i32.const 1) (i32.const 0))))
        (func (export "new-wrong-type")
          (drop (call $new-indirect (i32.const 2) (i32.const 0))))
        (func (export "new-past-the-end")
          (drop (call $new-indirect (i32.const 3) (i32.const 0))))
        (func (export "resume-self")
          (call $resume-later (call $thread-index)))
        (func (export "resume-twice")
          (local $thread i32)
          (local.set $thread (call $new-indirect (i32.const 0) (i32.const 0)))
          (call $resume-later (local.get $thread))
          (call $resume-later (local.get $thread)))
        (func (export "resume-unknown")
          (call $resume-later (i32.const 99)))

        (func (export "implicit-index") (result i32) (global.get $implicit-index))
        (func (export "created-index") (result i32) (global.get $created-index))
        (func (export "ran-before-yield") (result i32) (global.get $ran-before-yield))
        (func (export "context") (result i32) (global.get $context)))

      (core instance $i (instantiate $m
        (with "" (instance
          (export "task.return" (func $task-return))
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))))
        (with "libc" (instance $libc))))

      (func (export "spawn") async (result u32)
        (canon lift (core func $i "spawn") async (callback (core func $i "spawn-callback"))))
      (func (export "new-empty") (canon lift (core func $i "new-empty")))
      (func (export "new-wrong-type") (canon lift (core func $i "new-wrong-type")))
      (func (export "new-past-the-end") (canon lift (core func $i "new-past-the-end")))
      (func (export "resume-self") (canon lift (core func $i "resume-self")))
      (func (export "resume-twice") (canon lift (core func $i "resume-twice")))
      (func (export "resume-unknown") (canon lift (core func $i "resume-unknown")))
      (func (export "implicit-index") (result u32) (canon lift (core func $i "implicit-index")))
      (func (export "created-index") (result u32) (canon lift (core func $i "created-index")))
      (func (export "ran-before-yield") (result u32)
        (canon lift (core func $i "ran-before-yield")))
      (func (export "context") (result u32) (canon lift (core func $i "context"))))
    "#
);

/// A component whose `post-return`s each call one of the three
/// built-ins. The instance's may-leave flag is clear while a
/// `post-return` runs.
const POST_RETURN_CALLS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (table (export "__indirect_function_table") 1 funcref))
      (core instance $libc (instantiate $libc))

      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))
      (core func $resume-later (canon thread.resume-later))

      (core module $m
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func $new-indirect (param i32 i32) (result i32)))
        (import "" "thread.resume-later" (func $resume-later (param i32)))
        (import "libc" "__indirect_function_table" (table 1 funcref))
        (func $start (param i32))
        (elem (table 0) (i32.const 0) func $start)
        (func (export "run") (result i32) (i32.const 7))
        (func (export "index-after") (param i32)
          (drop (call $thread-index)))
        (func (export "new-after") (param i32)
          (drop (call $new-indirect (i32.const 0) (i32.const 0))))
        (func (export "resume-after") (param i32)
          (call $resume-later (i32.const 1))))

      (core instance $m (instantiate $m
        (with "" (instance
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))
          (export "thread.resume-later" (func $resume-later))))
        (with "libc" (instance $libc))))

      (func (export "index") (result u32)
        (canon lift (core func $m "run") (post-return (core func $m "index-after"))))
      (func (export "new") (result u32)
        (canon lift (core func $m "run") (post-return (core func $m "new-after"))))
      (func (export "resume") (result u32)
        (canon lift (core func $m "run") (post-return (core func $m "resume-after")))))
    "#
);

/// A component that declares `thread.index`, and nothing that would
/// need the threading gate otherwise.
const DECLARES_THREAD_INDEX: &[u8] = component!(
    r#"
    (component
      (core func $thread-index (canon thread.index))
      (core module $m (import "" "thread.index" (func (result i32))))
      (core instance $i (instantiate $m
        (with "" (instance (export "thread.index" (func $thread-index)))))))
    "#
);

/// An engine with the thread built-ins allowed.
fn threading_engine() -> Engine {
    let mut config = EngineConfig::new();
    config.wasm_component_model_threading(true);
    Engine::with_config(&config).expect("engine")
}

/// Instantiate `bytes` in a fresh store of an engine with the thread
/// built-ins allowed.
async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = threading_engine();
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

/// Call `name` and expect it to return one `u32`.
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str) -> u32 {
    match func(instance, name).call(store, &[]).await {
        Ok(values) => match values.first() {
            Some(Val::U32(value)) => *value,
            other => panic!("{name} answered {other:?}"),
        },
        Err(error) => panic!("{name} failed: {}", chain(&error)),
    }
}

/// Call `name` and expect it to fail, reporting the message.
async fn call_expecting_a_trap(store: &mut Store<()>, instance: &Instance, name: &str) -> String {
    match func(instance, name).call(store, &[]).await {
        Err(error) => chain(&error),
        Ok(values) => panic!("{name} returned {values:?} rather than trapping"),
    }
}

#[wcmp_macros::test]
async fn it_starts_a_thread_whose_start_function_takes_an_i32() {
    let (mut store, instance) = instantiate(THREADS).await;
    call_u32(&mut store, &instance, "spawn").await;
    assert_eq!(
        call_u32(&mut store, &instance, "context").await,
        42,
        "the start function received the context the guest passed"
    );
}

#[wcmp_macros::test]
async fn it_runs_a_thread_made_ready_in_a_later_turn_under_its_own_index() {
    let (mut store, instance) = instantiate(THREADS).await;
    let seen = call_u32(&mut store, &instance, "spawn").await;

    let implicit = call_u32(&mut store, &instance, "implicit-index").await;
    let created = call_u32(&mut store, &instance, "created-index").await;
    assert_eq!(
        implicit, 1,
        "the task's implicit thread took the first index"
    );
    assert_eq!(
        created, 2,
        "the thread the built-in created took the next one"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "ran-before-yield").await,
        0,
        "the thread had not run when the core function that made it ready \
         went on, so it runs in a later turn"
    );
    assert_eq!(
        seen, created,
        "`thread.index` answered the thread its own index while it ran"
    );
}

#[wcmp_macros::test]
async fn it_gives_a_finished_threads_index_to_the_next_thread() {
    let (mut store, instance) = instantiate(THREADS).await;
    call_u32(&mut store, &instance, "spawn").await;
    call_u32(&mut store, &instance, "spawn").await;
    assert_eq!(
        call_u32(&mut store, &instance, "created-index").await,
        2,
        "the first call's threads left the table, so the second call's \
         thread takes the same index"
    );
}

#[wcmp_macros::test]
async fn it_fails_an_empty_table_entry_with_wasmtimes_message() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "new-empty").await;
    assert!(
        message.contains("the start function index points to an uninitialized function"),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_start_function_of_another_type_with_wasmtimes_message() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "new-wrong-type").await;
    assert!(
        message.contains(
            "start function does not match expected type \
             (currently only `(i32) -> ()` is supported)"
        ),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_table_index_past_the_end_with_wasmtimes_message() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "new-past-the-end").await;
    assert!(
        message.contains("undefined element: out of bounds table access"),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_resume_the_running_thread() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "resume-self").await;
    assert!(
        message.contains("cannot resume thread which is not suspended"),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_resume_a_thread_that_is_already_ready() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "resume-twice").await;
    assert!(
        message.contains("cannot resume thread which is not suspended"),
        "got {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_resume_an_index_that_names_no_thread() {
    let (mut store, instance) = instantiate(THREADS).await;
    let message = call_expecting_a_trap(&mut store, &instance, "resume-unknown").await;
    assert!(message.contains("unknown handle index 99"), "got {message}");
}

#[wcmp_macros::test]
async fn it_traps_each_built_in_when_the_instance_may_not_be_left() {
    for name in ["index", "new", "resume"] {
        let (mut store, instance) = instantiate(POST_RETURN_CALLS).await;
        let message = call_expecting_a_trap(&mut store, &instance, name).await;
        assert!(
            message.contains("cannot leave component instance"),
            "the built-in `{name}`'s post-return calls was refused with the \
             cannot-leave cause, got {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_refuses_a_thread_built_in_as_unsupported_with_the_gate_off() {
    let engine = Engine::new().expect("engine");
    let err = Component::new(&engine, DECLARES_THREAD_INDEX)
        .await
        .expect_err("the gate is off by default");
    assert!(
        matches!(&err, Error::Unsupported { feature }
            if feature.contains("requires the component model threading feature")),
        "expected the closed gate to be unsupported, got {err:?}"
    );
}

/// One component instance with a 64-bit memory, whose table holds a
/// start function of type `(i64) -> ()` at entry 1. The start
/// function stores its context at address 0 and its own index at
/// address 8, where `context` and `seen-index` read them back.
///
/// The component declares `thread.new-indirect` of the one start
/// type the validator admits, which is what gives the instance its
/// extracted table. The test builds the `i64` form of the built-in
/// over that same table, as a component could declare it if the
/// validator admitted it.
const I64_START: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") i64 1)
        (table (export "__indirect_function_table") 2 funcref))
      (core instance $libc (instantiate $libc))

      (core func $thread-index (canon thread.index))
      (core type $start-ty (func (param i32)))
      (alias core export $libc "__indirect_function_table" (core table $table))
      (core func $new-indirect (canon thread.new-indirect $start-ty (core table $table)))

      (core module $m
        (import "" "thread.index" (func $thread-index (result i32)))
        (import "" "thread.new-indirect" (func (param i32 i32) (result i32)))
        (import "libc" "memory" (memory i64 1))
        (import "libc" "__indirect_function_table" (table 2 funcref))
        (func $start (param i64)
          (i64.store (i64.const 0) (local.get 0))
          (i32.store (i64.const 8) (call $thread-index)))
        (elem (table 0) (i32.const 1) func $start)
        (func (export "context") (result i64) (i64.load (i64.const 0)))
        (func (export "seen-index") (result i32) (i32.load (i64.const 8))))

      (core instance $i (instantiate $m
        (with "" (instance
          (export "thread.index" (func $thread-index))
          (export "thread.new-indirect" (func $new-indirect))))
        (with "libc" (instance $libc))))

      (func (export "context") (result u64) (canon lift (core func $i "context")))
      (func (export "seen-index") (result u32) (canon lift (core func $i "seen-index"))))
    "#
);

#[wcmp_macros::test]
async fn it_starts_a_thread_whose_start_function_takes_an_i64_in_a_64_bit_memory() {
    use core::task::Waker;

    use wasm_runtime_layer::Val as RuntimeVal;

    use crate::abi::layout::FlatType;
    use crate::executor::ir::{CoreParameter, CoreSignature};
    use crate::executor::{build_thread_new_indirect, build_thread_resume_later};
    use crate::internal::FuncInternal;
    use crate::store::StoreInternalExt;

    // An address past four gigabytes, which only an `i64` carries.
    const CONTEXT: i64 = 0x1_0000_0010;

    let (mut store, instance) = instantiate(I64_START).await;
    let reader = func(&instance, "context");
    let abi_state = reader.abi_state().clone();
    let component_instance = reader.options().instance;
    let instance_id = abi_state
        .lock()
        .expect("the instance's ABI state")
        .component_instances[component_instance];

    // The `i64` form of the two built-ins, over the table the
    // instantiation extracted into runtime-table slot 0.
    let new_indirect = build_thread_new_indirect(
        &mut store.internal().context(),
        component_instance,
        0,
        &CoreSignature {
            params: vec![
                CoreParameter::Value(FlatType::I32),
                CoreParameter::Value(FlatType::I64),
            ],
            results: vec![FlatType::I32],
        },
        abi_state.clone(),
    );
    let resume_later = build_thread_resume_later(
        &mut store.internal().context(),
        component_instance,
        &CoreSignature {
            params: vec![CoreParameter::Value(FlatType::I32)],
            results: Vec::new(),
        },
        abi_state,
    );

    // A task of the instance is running, as it would be while the
    // guest's own call ran the built-ins.
    let task = {
        let mut guard = store.internal().tables().lock().expect("handle tables");
        let task = guard
            .tasks
            .push_task(None, None, instance_id)
            .expect("room under the record cap");
        guard.tasks.start_task(task);
        task
    };
    let mut index = [RuntimeVal::I32(0)];
    new_indirect
        .call(
            store.internal().inner_mut(),
            &[RuntimeVal::I32(1), RuntimeVal::I64(CONTEXT)],
            &mut index,
        )
        .expect("the `(i64) -> ()` start function is admitted");
    let [RuntimeVal::I32(index)] = index else {
        panic!("the built-in answered {index:?}");
    };
    resume_later
        .call(
            store.internal().inner_mut(),
            &[RuntimeVal::I32(index)],
            &mut [],
        )
        .expect("the new thread is suspended");
    store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .leave_task_scope(task);

    // The thread was queued as a resumption after a yield, which a
    // driver's turn hands the host executor first and runs at the
    // top of the next turn.
    for _ in 0..2 {
        store.internal().turn(Waker::noop()).expect("a turn");
    }

    let context = match func(&instance, "context").call(&mut store, &[]).await {
        Ok(values) => values.first().cloned(),
        Err(error) => panic!("`context` failed: {}", chain(&error)),
    };
    assert_eq!(
        context,
        Some(Val::U64(CONTEXT as u64)),
        "the start function received the whole 64-bit context"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "seen-index").await,
        index as u32,
        "the thread ran under the index the built-in answered"
    );
}
