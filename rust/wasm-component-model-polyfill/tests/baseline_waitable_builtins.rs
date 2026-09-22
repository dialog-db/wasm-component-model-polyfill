//! Baseline tests for the waitable set built-ins.
//!
//! A guest reaches the store's waitable set records through five
//! built-ins: `waitable-set.new`, `waitable-set.wait`,
//! `waitable-set.poll`, `waitable-set.drop`, and `waitable.join`.
//! The components here call them from a synchronous export, which is
//! a task that must not block, and from a `realloc`, where the
//! instance's may-leave flag is clear.
//!
//! One more component calls the wait from the far end of a
//! synchronous call chain, in an `async`-typed export that is
//! allowed to block. The chain's caller is not, so the wait fails
//! with the cannot-block cause and not as a deadlock.
//!
//! In this design the subtask is the one waitable kind the polyfill
//! builds, and nothing in these components starts a subtask. The
//! waitables the tests join and deliver are therefore inserted
//! through the store's records, the way the feature that adds the
//! first waitable kind will produce them.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component whose synchronous exports are the five waitable set
/// built-ins, with two more that read and write the memory the wait
/// and the poll write their payloads through.
///
/// The memory sits in a module of its own, instantiated before the
/// built-ins are declared, because the built-ins name it and the
/// module that calls them imports them.
const SET_BUILTINS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0)))
        (func (export "poke") (param i32 i32) (i32.store (local.get 0) (local.get 1))))
      (core instance $libc (instantiate $libc))

      (core func $new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core func $drop-set (canon waitable-set.drop))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "waitable-set.new" (func $new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable-set.drop" (func $drop-set (param i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func (export "new-set") (result i32) (call $new))
        (func (export "wait") (param i32) (result i32)
          (call $wait (local.get 0) (i32.const 0)))
        (func (export "poll") (param i32) (result i32)
          (call $poll (local.get 0) (i32.const 0)))
        (func (export "wait-at") (param i32 i32) (result i32)
          (call $wait (local.get 0) (local.get 1)))
        (func (export "poll-at") (param i32 i32) (result i32)
          (call $poll (local.get 0) (local.get 1)))
        (func (export "drop-set") (param i32) (call $drop-set (local.get 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1))))
      (core instance $m (instantiate $m (with "" (instance
        (export "waitable-set.new" (func $new))
        (export "waitable-set.wait" (func $wait))
        (export "waitable-set.poll" (func $poll))
        (export "waitable-set.drop" (func $drop-set))
        (export "waitable.join" (func $join))))))

      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "wait") (param "s" u32) (result u32) (canon lift (core func $m "wait")))
      (func (export "poll") (param "s" u32) (result u32) (canon lift (core func $m "poll")))
      (func (export "wait-at") (param "s" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "wait-at")))
      (func (export "poll-at") (param "s" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "poll-at")))
      (func (export "drop-set") (param "s" u32) (canon lift (core func $m "drop-set")))
      (func (export "join") (param "w" u32) (param "s" u32)
        (canon lift (core func $m "join")))
      (func (export "peek") (param "p" u32) (result u32)
        (canon lift (core func $libc "peek")))
      (func (export "poke") (param "p" u32) (param "v" u32)
        (canon lift (core func $libc "poke"))))
    "#
);

/// A component whose `realloc` calls one of the five built-ins. The
/// `select` export says which; the `run` export takes a string, so
/// the host's argument lowering calls the `realloc`, and the
/// instance's may-leave flag is clear while it runs.
const REALLOC_CALLS_BUILTINS: &[u8] = component!(
    r#"
    (component
      (core module $mem (memory (export "memory") 1))
      (core instance $mem (instantiate $mem))

      (core func $new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $mem "memory"))))
      (core func $poll (canon waitable-set.poll (memory (core memory $mem "memory"))))
      (core func $drop-set (canon waitable-set.drop))
      (core func $join (canon waitable.join))

      (core module $m
        (import "" "waitable-set.new" (func $new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (import "" "waitable-set.drop" (func $drop-set (param i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $which (mut i32) (i32.const 0))
        (func (export "select") (param i32) (global.set $which (local.get 0)))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (if (i32.eq (global.get $which) (i32.const 0))
            (then (drop (call $new))))
          (if (i32.eq (global.get $which) (i32.const 1))
            (then (drop (call $wait (i32.const 0) (i32.const 0)))))
          (if (i32.eq (global.get $which) (i32.const 2))
            (then (drop (call $poll (i32.const 0) (i32.const 0)))))
          (if (i32.eq (global.get $which) (i32.const 3))
            (then (call $drop-set (i32.const 0))))
          (if (i32.eq (global.get $which) (i32.const 4))
            (then (call $join (i32.const 0) (i32.const 0))))
          (i32.const 16))
        (func (export "run") (param i32 i32)))
      (core instance $m (instantiate $m (with "" (instance
        (export "waitable-set.new" (func $new))
        (export "waitable-set.wait" (func $wait))
        (export "waitable-set.poll" (func $poll))
        (export "waitable-set.drop" (func $drop-set))
        (export "waitable.join" (func $join))))))

      (func (export "select") (param "w" u32) (canon lift (core func $m "select")))
      (func (export "run") (param "x" string)
        (canon lift (core func $m "run")
          (realloc (core func $m "realloc"))
          (memory (core memory $mem "memory")))))
    "#
);

/// Instantiate `binary` in a fresh store with nothing registered.
async fn instantiate(binary: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, binary)
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

/// Call `name` with `args` and report the one value it returned, or
/// the message of the trap it raised.
async fn call(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
    args: &[Val],
) -> Result<Option<Val>, String> {
    func(instance, name)
        .call(store, args)
        .await
        .map(|values| values.first().cloned())
        .map_err(|error| chain(&error))
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
async fn call_u32(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> u32 {
    match call(store, instance, name, args).await {
        Ok(Some(Val::U32(value))) => value,
        other => panic!("{name} answered {other:?}"),
    }
}

/// Call `name` and expect it to trap, reporting the message.
async fn call_trap(store: &mut Store<()>, instance: &Instance, name: &str, args: &[Val]) -> String {
    match call(store, instance, name, args).await {
        Err(message) => message,
        Ok(value) => panic!("{name} returned {value:?} rather than trapping"),
    }
}

/// The handle table the instance's exports resolve their indices
/// against, read the way a built-in reads it: through the canon
/// options of a declaration of the same component instance.
///
/// The identity has no name outside the crate, so the lookup is a
/// macro rather than a function: every use of it binds the value and
/// hands it straight back to the store's records.
macro_rules! handle_table {
    ($instance:expr) => {{
        let export = func($instance, "wait");
        let state = export.abi_state.lock().expect("the instance's ABI state");
        state.handle_tables[export.options.instance]
    }};
}

/// Put a subtask in the instance's own table and report the index the
/// table gave it. The entry is a waitable and not a waitable set, so
/// it is the index a built-in that names a set traps on.
fn subtask_in_table(store: &mut Store<()>, instance: &Instance) -> u32 {
    let table = handle_table!(instance);
    let mut guard = store.tables().lock().expect("handle tables");
    let subtask = guard.tasks.insert_subtask();
    guard.insert_subtask(table, subtask)
}

/// Put a returned subtask in the instance's own table, joined to the
/// set `set_index` names and holding its ready event, and report the
/// index the table gave it.
///
/// This is the delivery path of a wait and of a poll, built with the
/// one waitable kind the store makes: nothing in these components
/// starts a subtask, so the test inserts one through the records.
fn ready_subtask_in_set(store: &mut Store<()>, instance: &Instance, set_index: u32) -> u32 {
    let table = handle_table!(instance);
    let mut guard = store.tables().lock().expect("handle tables");
    let set = guard
        .waitable_set_from_handle(table, set_index)
        .expect("the guest's index names the set it created");
    let subtask = guard.tasks.insert_subtask();
    let waitable = guard.tasks.subtask_waitable(subtask);
    let subtask_index = guard.insert_subtask(table, subtask);
    guard
        .tasks
        .join_waitable_set(waitable, Some(set))
        .expect("the subtask joins the set");
    guard
        .tasks
        .subtask_returned(subtask)
        .expect("the call the subtask names returned");
    guard
        .tasks
        .record_subtask_event(subtask, subtask_index)
        .expect("the subtask is ready");
    subtask_index
}

#[wcmp_macros::test]
async fn it_returns_an_event_from_a_wait_on_a_set_that_already_holds_one() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = ready_subtask_in_set(&mut store, &instance, set_index);

    let code = call_u32(&mut store, &instance, "wait", &[Val::U32(set_index)]).await;

    assert_eq!(
        code, 1,
        "the wait returned the subtask event's code without blocking"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        subtask_index,
        "the first payload is the waitable's index in the instance's table"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(4)]).await,
        2,
        "the second payload is the state the subtask resolved to, written at \
         the pointer plus four"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_wait_on_an_empty_set_with_the_cannot_block_cause() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;

    let message = call_trap(&mut store, &instance, "wait", &[Val::U32(set_index)]).await;

    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "a synchronous export must return before its instance may block: {message}"
    );
}

/// Two components in a synchronous call chain, where the callee's
/// export is typed `async` and lifted synchronously.
///
/// The host calls `$Outer`'s `f`, a synchronous export, so `$Outer`
/// is inside a call that must return. `f` calls `$Inner`'s `g`
/// through a synchronous lowering. `g` is typed `async`, so the
/// enter intrinsic leaves `$Inner` free to block, and `g` blocks on
/// a `waitable-set.wait` over a set it just created and nothing
/// joined. Nothing in the store can ever fill that set.
const CHAIN_INTO_ASYNC: &[u8] = component!(
    r#"
    (component
      (component $Inner
        (core module $mem (memory (export "memory") 1))
        (core instance $mem (instantiate $mem))
        (core func $new (canon waitable-set.new))
        (core func $wait (canon waitable-set.wait (memory (core memory $mem "memory"))))
        (core module $m
          (import "" "waitable-set.new" (func $new (result i32)))
          (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
          (func (export "g")
            (drop (call $wait (call $new) (i32.const 0)))))
        (core instance $i (instantiate $m (with "" (instance
          (export "waitable-set.new" (func $new))
          (export "waitable-set.wait" (func $wait))))))
        (func (export "g") async (canon lift (core func $i "g"))))
      (component $Outer
        (import "inner" (instance $inner (export "g" (func async))))
        (core func $g (canon lower (func $inner "g")))
        (core module $m
          (import "" "g" (func $g))
          (func (export "f") (call $g)))
        (core instance $i (instantiate $m
          (with "" (instance (export "g" (func $g))))))
        (func (export "f") (canon lift (core func $i "f"))))
      (instance $inner (instantiate $Inner))
      (instance $outer (instantiate $Outer (with "inner" (instance $inner))))
      (export "f" (func $outer "f")))
    "#
);

#[wcmp_macros::test]
async fn it_fails_a_wait_reached_through_a_synchronous_call_with_the_cannot_block_cause() {
    let (mut store, instance) = instantiate(CHAIN_INTO_ASYNC).await;

    let message = call_trap(&mut store, &instance, "f", &[]).await;

    assert!(
        message.contains("cannot block a synchronous task before returning"),
        "the `async`-typed callee is allowed to block, but its synchronous \
         caller must return, so a store that goes idle under it fails by the \
         caller's rule rather than as a deadlock: {message}"
    );
}

#[wcmp_macros::test]
async fn it_answers_a_poll_of_an_empty_set_with_the_none_code_and_two_zero_words() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    for (offset, value) in [(0u32, 0x1111_2222u32), (4, 0x3333_4444)] {
        call(
            &mut store,
            &instance,
            "poke",
            &[Val::U32(offset), Val::U32(value)],
        )
        .await
        .expect("the guest writes its own memory");
    }

    let code = call_u32(&mut store, &instance, "poll", &[Val::U32(set_index)]).await;

    assert_eq!(code, 0, "an empty set answers a poll with the none code");
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        0,
        "the none event's first payload is zero, and the poll writes it \
         at the pointer over what the guest left there"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(4)]).await,
        0,
        "and its second payload at the pointer plus four"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_drop_a_set_that_still_holds_a_waitable() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    {
        let table = handle_table!(&instance);
        let mut guard = store.tables().lock().expect("handle tables");
        let set = guard
            .waitable_set_from_handle(table, set_index)
            .expect("the set the guest created");
        let subtask = guard.tasks.insert_subtask();
        let waitable = guard.tasks.subtask_waitable(subtask);
        guard
            .tasks
            .join_waitable_set(waitable, Some(set))
            .expect("the subtask joins the set");
    }

    let message = call_trap(&mut store, &instance, "drop-set", &[Val::U32(set_index)]).await;

    assert!(
        message.contains("cannot drop waitable set with waitables in it"),
        "a set the guest still holds a waitable in cannot be dropped: {message}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_drop_a_set_a_thread_waits_on() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    {
        let table = handle_table!(&instance);
        let mut guard = store.tables().lock().expect("handle tables");
        let set = guard
            .waitable_set_from_handle(table, set_index)
            .expect("the set the guest created");
        // A thread of another task, parked on the set: what a
        // blocking wait leaves behind while it is suspended.
        let other = guard.tasks.insert_instance();
        let task = guard.tasks.create_task(None, None, other);
        let thread = guard
            .tasks
            .task(task)
            .expect("the task record")
            .implicit_thread;
        guard
            .tasks
            .begin_wait(set, thread)
            .expect("the thread parks on the set");
    }

    let message = call_trap(&mut store, &instance, "drop-set", &[Val::U32(set_index)]).await;

    assert!(
        message.contains("cannot drop waitable set with waiters"),
        "a set a thread waits on cannot be dropped: {message}"
    );
}

#[wcmp_macros::test]
async fn it_removes_the_set_entry_when_the_set_is_dropped() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;

    call(&mut store, &instance, "drop-set", &[Val::U32(set_index)])
        .await
        .expect("an empty set no thread waits on drops");

    let table = handle_table!(&instance);
    let guard = store.tables().lock().expect("handle tables");
    assert!(
        guard.entry(table, set_index).is_none(),
        "the entry left the instance's handle table"
    );
    assert_eq!(
        guard.tasks.waitable_set_count(),
        0,
        "and the record left the store"
    );
}

#[wcmp_macros::test]
async fn it_removes_a_waitable_from_its_set_when_the_set_index_is_zero() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = {
        let table = handle_table!(&instance);
        let mut guard = store.tables().lock().expect("handle tables");
        let subtask = guard.tasks.insert_subtask();
        guard.insert_subtask(table, subtask)
    };

    call(
        &mut store,
        &instance,
        "join",
        &[Val::U32(subtask_index), Val::U32(set_index)],
    )
    .await
    .expect("the waitable joins the set");
    {
        let table = handle_table!(&instance);
        let guard = store.tables().lock().expect("handle tables");
        let waitable = guard
            .waitable_from_handle(table, subtask_index)
            .expect("the subtask the guest joined");
        let set = guard
            .waitable_set_from_handle(table, set_index)
            .expect("the set the guest created");
        assert_eq!(
            guard.tasks.waitable_set_of(waitable).expect("the record"),
            Some(set),
            "the waitable names the set it joined"
        );
    }

    call(
        &mut store,
        &instance,
        "join",
        &[Val::U32(subtask_index), Val::U32(0)],
    )
    .await
    .expect("a set index of zero takes the waitable out of its set");

    let table = handle_table!(&instance);
    let guard = store.tables().lock().expect("handle tables");
    let waitable = guard
        .waitable_from_handle(table, subtask_index)
        .expect("the subtask is still in the table");
    assert_eq!(
        guard.tasks.waitable_set_of(waitable).expect("the record"),
        None,
        "the waitable is in no set"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_wait_whose_index_is_not_a_set() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let subtask_index = subtask_in_table(&mut store, &instance);

    let message = call_trap(&mut store, &instance, "wait", &[Val::U32(subtask_index)]).await;

    assert!(
        message.contains("is not a waitable-set"),
        "a wait resolves its index against the instance's table and a subtask \
         entry is not a set: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_poll_whose_index_is_not_a_set() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let subtask_index = subtask_in_table(&mut store, &instance);

    let message = call_trap(&mut store, &instance, "poll", &[Val::U32(subtask_index)]).await;

    assert!(
        message.contains("is not a waitable-set"),
        "a poll checks the index the wait checks, before it asks whether the \
         set holds an event: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_whose_index_is_not_a_set() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let subtask_index = subtask_in_table(&mut store, &instance);

    let message = call_trap(
        &mut store,
        &instance,
        "drop-set",
        &[Val::U32(subtask_index)],
    )
    .await;

    assert!(
        message.contains("is not a waitable-set"),
        "a drop names a set, so a subtask entry traps rather than leaving the \
         table: {message}"
    );
    let table = handle_table!(&instance);
    assert!(
        store
            .tables()
            .lock()
            .expect("handle tables")
            .entry(table, subtask_index)
            .is_some(),
        "the entry the drop refused is still in the instance's table"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_join_whose_first_index_is_not_a_waitable() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;

    // A waitable set is not a waitable, so it cannot be the thing
    // that joins one.
    let message = call_trap(
        &mut store,
        &instance,
        "join",
        &[Val::U32(set_index), Val::U32(set_index)],
    )
    .await;

    assert!(
        message.contains("is not a waitable"),
        "the first index must name a waitable: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_join_whose_second_index_is_not_a_set() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let subtask_index = subtask_in_table(&mut store, &instance);

    let message = call_trap(
        &mut store,
        &instance,
        "join",
        &[Val::U32(subtask_index), Val::U32(subtask_index)],
    )
    .await;

    assert!(
        message.contains("is not a waitable-set"),
        "the second index must name a waitable set: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_join_of_a_waitable_that_has_a_synchronous_waiter() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = {
        let table = handle_table!(&instance);
        let mut guard = store.tables().lock().expect("handle tables");
        let subtask = guard.tasks.insert_subtask();
        let waitable = guard.tasks.subtask_waitable(subtask);
        let index = guard.insert_subtask(table, subtask);
        guard
            .tasks
            .begin_synchronous_wait(waitable)
            .expect("a thread waits on the subtask on its own");
        index
    };

    let message = call_trap(
        &mut store,
        &instance,
        "join",
        &[Val::U32(subtask_index), Val::U32(set_index)],
    )
    .await;

    assert!(
        message.contains("waitable cannot be used synchronously while added to a waitable set"),
        "a waitable a thread waits on alone cannot join a set: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_every_built_in_a_realloc_calls_with_the_cannot_leave_cause() {
    // The `select` export says which built-in the `realloc` calls,
    // in the order the five are declared.
    for (which, built_in) in [
        (0u32, "waitable-set.new"),
        (1, "waitable-set.wait"),
        (2, "waitable-set.poll"),
        (3, "waitable-set.drop"),
        (4, "waitable.join"),
    ] {
        let (mut store, instance) = instantiate(REALLOC_CALLS_BUILTINS).await;
        call(&mut store, &instance, "select", &[Val::U32(which)])
            .await
            .expect("the guest records which built-in its realloc calls");

        // Lowering the string argument calls the `realloc`, and the
        // instance may not be left while it runs.
        let message = call_trap(
            &mut store,
            &instance,
            "run",
            &[Val::String("hi".to_owned())],
        )
        .await;

        assert!(
            message.contains("cannot leave component instance"),
            "{built_in} from a realloc must fail with the cannot-leave cause: {message}"
        );
    }
}

#[wcmp_macros::test]
async fn it_writes_the_event_payloads_at_a_non_zero_aligned_pointer() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = ready_subtask_in_set(&mut store, &instance, set_index);

    let code = call_u32(
        &mut store,
        &instance,
        "wait-at",
        &[Val::U32(set_index), Val::U32(16)],
    )
    .await;

    assert_eq!(code, 1, "the wait returned the subtask event's code");
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(16)]).await,
        subtask_index,
        "the first payload landed at the pointer the guest passed"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(20)]).await,
        2,
        "the second payload landed at that pointer plus four"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_wait_whose_event_pointer_is_not_aligned() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    ready_subtask_in_set(&mut store, &instance, set_index);

    let message = call_trap(
        &mut store,
        &instance,
        "wait-at",
        &[Val::U32(set_index), Val::U32(1)],
    )
    .await;

    assert!(
        message.contains("event pointer not aligned to 4"),
        "each payload is stored as a u32, so the pointer must be a multiple \
         of four: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_poll_whose_event_pointer_is_not_aligned() {
    let (mut store, instance) = instantiate(SET_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    ready_subtask_in_set(&mut store, &instance, set_index);

    let message = call_trap(
        &mut store,
        &instance,
        "poll-at",
        &[Val::U32(set_index), Val::U32(1)],
    )
    .await;

    assert!(
        message.contains("event pointer not aligned to 4"),
        "the poll writes the pair the wait writes, so it checks the same \
         alignment: {message}"
    );
}

/// The memory the built-ins of [`SET_BUILTINS`] write through is one
/// page, so a pointer at its end leaves too little room for the pair
/// of `u32` values an event is written as.
const MEMORY_BYTES: u32 = 65_536;

#[wcmp_macros::test]
async fn it_traps_a_poll_of_an_empty_set_on_every_pointer_a_wait_traps_on() {
    // Three pointers and the cause each one raises: one that is not
    // a multiple of four, one two bytes from the end of the memory,
    // which is not a multiple of four either and so fails the same
    // way, and one four bytes from the end, which is aligned and
    // fails when the pair leaves the memory halfway through.
    //
    // The whole message cannot be compared, because the substrate
    // puts the core function the trap came from in front of the
    // cause and the poll and the wait are two different functions.
    for (pointer, cause) in [
        (1, "event pointer not aligned to 4"),
        (MEMORY_BYTES - 2, "event pointer not aligned to 4"),
        (MEMORY_BYTES - 4, "guest memory access failed"),
    ] {
        let (mut store, instance) = instantiate(SET_BUILTINS).await;
        let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
        let polled = call_trap(
            &mut store,
            &instance,
            "poll-at",
            &[Val::U32(set_index), Val::U32(pointer)],
        )
        .await;

        let (mut store, instance) = instantiate(SET_BUILTINS).await;
        let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
        ready_subtask_in_set(&mut store, &instance, set_index);
        let waited = call_trap(
            &mut store,
            &instance,
            "wait-at",
            &[Val::U32(set_index), Val::U32(pointer)],
        )
        .await;

        assert!(
            waited.contains(cause),
            "a wait that has an event to deliver fails at pointer {pointer} \
             with the cause the pointer earns: {waited}"
        );
        assert!(
            polled.contains(cause),
            "a poll of an empty set writes the none event's two zero words \
             through the pointer, so pointer {pointer} fails it for the \
             reason it fails the wait: {polled}"
        );
    }
}
