//! Baseline tests for subtask events and the `subtask.drop`
//! built-in.
//!
//! A subtask is the record of one call out through an import. It
//! enters the caller's handle table when the lower returns with the
//! call still running, it holds one pending event slot, and
//! `subtask.drop` is what takes its entry away again.
//!
//! Two rules of the record are read back here. The event a subtask
//! delivers carries the state the record is in *at delivery*, not the
//! state it was in when the slot was filled, so a `STARTED` that
//! nothing took reads as `RETURNED` once the call has returned. And a
//! subtask cannot be dropped until its resolution has been delivered,
//! because the handles the call borrowed are given back by that same
//! delivery and the guest has had no notice of the outcome until
//! then.
//!
//! Two components read them back. The first calls a host `async`
//! function through an asynchronous lower and drops the subtask the
//! call left behind, which is the whole path end to end. The second
//! reaches the store's records directly, the way the waitable
//! baselines do, so that each state a subtask can be dropped in — and
//! each index that is not a subtask at all — can be arranged exactly.
//! A third component calls the built-in from a `realloc`, where the
//! instance's may-leave flag is clear.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::internal::FuncInternal;
use crate::store::StoreInternalExt;
use crate::{Accessor, Component, Engine, Error, Func, Instance, Linker, Store, Val};
use wcmp_macros::component;

/// A component that calls a host `async` function through an
/// asynchronous lower and drops the subtask the call left behind.
///
/// `run` is lifted `async` with a callback. It calls the import,
/// joins the subtask it is given to a set of its own, and returns the
/// wait word; the callback receives the subtask event, drops the
/// subtask, and returns the result the lowering wrote. The `select`
/// export moves the drop: mode one drops the subtask inside `run`,
/// while the host's future is still pending, and mode two never drops
/// it at all.
const DROPS_A_HOST_SUBTASK: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $subtask-drop (canon subtask.drop))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $subtask-drop (param i32)))
        (global $mode (mut i32) (i32.const 0))
        (global $subtask (mut i32) (i32.const 0))
        (global $set (mut i32) (i32.const 0))
        (func (export "select") (param i32) (global.set $mode (local.get 0)))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (if (i32.ne (i32.and (local.get $status) (i32.const 0xf)) (i32.const 1))
            (then unreachable))
          (global.set $subtask (i32.shr_u (local.get $status) (i32.const 4)))
          (global.set $set (call $set-new))
          (call $join (global.get $subtask) (global.get $set))
          (if (i32.eq (global.get $mode) (i32.const 1))
            (then (call $subtask-drop (global.get $subtask))))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (if (i32.eqz (global.get $mode))
            (then (call $subtask-drop (global.get $subtask))))
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0))
        (func (export "subtask") (result i32) (global.get $subtask)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))
          (export "subtask.drop" (func $subtask-drop))))))
      (func (export "select") (param "m" u32) (canon lift (core func $m "select")))
      (func (export "subtask") (result u32) (canon lift (core func $m "subtask")))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A component whose synchronous exports are the built-ins a subtask
/// is waited on and dropped through, with one more that reads the
/// memory the wait writes its payloads into.
///
/// Nothing here starts a subtask of its own: the tests insert one
/// through the store's records, so that the state the record is in
/// when the guest reaches it is exactly the state under test.
const SUBTASK_BUILTINS: &[u8] = component!(
    r#"
    (component
      (core module $libc
        (memory (export "memory") 1)
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $libc (instantiate $libc))

      (core func $new (canon waitable-set.new))
      (core func $wait (canon waitable-set.wait (memory (core memory $libc "memory"))))
      (core func $join (canon waitable.join))
      (core func $drop-subtask (canon subtask.drop))

      (core module $m
        (import "" "waitable-set.new" (func $new (result i32)))
        (import "" "waitable-set.wait" (func $wait (param i32 i32) (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "subtask.drop" (func $drop-subtask (param i32)))
        (func (export "new-set") (result i32) (call $new))
        (func (export "wait") (param i32) (result i32)
          (call $wait (local.get 0) (i32.const 0)))
        (func (export "join") (param i32 i32) (call $join (local.get 0) (local.get 1)))
        (func (export "drop-subtask") (param i32) (call $drop-subtask (local.get 0))))
      (core instance $m (instantiate $m (with "" (instance
        (export "waitable-set.new" (func $new))
        (export "waitable-set.wait" (func $wait))
        (export "waitable.join" (func $join))
        (export "subtask.drop" (func $drop-subtask))))))

      (func (export "new-set") (result u32) (canon lift (core func $m "new-set")))
      (func (export "wait") (param "s" u32) (result u32) (canon lift (core func $m "wait")))
      (func (export "join") (param "w" u32) (param "s" u32)
        (canon lift (core func $m "join")))
      (func (export "drop-subtask") (param "s" u32)
        (canon lift (core func $m "drop-subtask")))
      (func (export "peek") (param "p" u32) (result u32)
        (canon lift (core func $libc "peek"))))
    "#
);

/// A component whose `realloc` calls `subtask.drop`. The `run` export
/// takes a string, so the host's argument lowering calls the
/// `realloc`, and the instance's may-leave flag is clear while it
/// runs.
const REALLOC_DROPS_A_SUBTASK: &[u8] = component!(
    r#"
    (component
      (core module $mem (memory (export "memory") 1))
      (core instance $mem (instantiate $mem))

      (core func $drop-subtask (canon subtask.drop))

      (core module $m
        (import "" "subtask.drop" (func $drop-subtask (param i32)))
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
          (call $drop-subtask (i32.const 1))
          (i32.const 16))
        (func (export "run") (param i32 i32)))
      (core instance $m (instantiate $m (with "" (instance
        (export "subtask.drop" (func $drop-subtask))))))

      (func (export "run") (param "x" string)
        (canon lift (core func $m "run")
          (realloc (core func $m "realloc"))
          (memory (core memory $mem "memory")))))
    "#
);

/// A future that is pending the first time it is polled and ready
/// afterwards. One poll apart is all a call needs to be a subtask:
/// the trampoline's own poll is the first, so the call starts, and
/// the next turn's poll completes it.
struct PendingOnce {
    polled: bool,
    value: u32,
}

impl Future for PendingOnce {
    type Output = Result<u32, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.polled {
            return Poll::Ready(Ok(this.value));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

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

/// Instantiate [`DROPS_A_HOST_SUBTASK`] with an `answer` whose future
/// is ready one poll after the call, so that every call it makes
/// starts rather than returning at once.
async fn host_caller() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, DROPS_A_HOST_SUBTASK)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
            PendingOnce {
                polled: false,
                value: x * 2,
            }
        })
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
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

/// How many subtask records the store holds.
fn subtask_count(store: &Store<()>) -> usize {
    store
        .internal_ref()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .subtask_count()
}

/// The handle table the instance's exports resolve their indices
/// against, read the way a built-in reads it: through the canon
/// options of a declaration of the same component instance.
///
/// The identity has no name outside the crate, so the lookup is a
/// macro rather than a function: every use of it binds the value and
/// hands it straight back to the store's records.
macro_rules! handle_table {
    ($instance:expr, $export:literal) => {{
        let export = func($instance, $export);
        let state = export.abi_state().lock().expect("the instance's ABI state");
        state.handle_tables[export.options().instance]
    }};
}

/// Put a subtask in the instance's own table, joined to the set
/// `set_index` names, and report the index the table gave it. The
/// subtask is left in its starting state: the caller of each test
/// moves it on from there.
fn subtask_in_set(store: &mut Store<()>, instance: &Instance, set_index: u32) -> u32 {
    let table = handle_table!(instance, "wait");
    let mut guard = store.internal().tables().lock().expect("handle tables");
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
    subtask_index
}

/// Move the subtask at `subtask_index` to its returned state, without
/// touching its pending event slot.
fn subtask_returns(store: &mut Store<()>, instance: &Instance, subtask_index: u32) {
    let table = handle_table!(instance, "wait");
    let mut guard = store.internal().tables().lock().expect("handle tables");
    let subtask = guard
        .subtask_from_handle(table, subtask_index)
        .expect("the index names the subtask the test inserted");
    guard
        .tasks
        .subtask_returned(subtask)
        .expect("the call the subtask names returned");
}

/// Move the subtask at `subtask_index` to its started state, which
/// fills its pending event slot because the guest already holds the
/// entry.
fn subtask_starts(store: &mut Store<()>, instance: &Instance, subtask_index: u32) {
    let table = handle_table!(instance, "wait");
    let mut guard = store.internal().tables().lock().expect("handle tables");
    let subtask = guard
        .subtask_from_handle(table, subtask_index)
        .expect("the index names the subtask the test inserted");
    guard.tasks.start_subtask(subtask);
}

#[wcmp_macros::test]
async fn it_delivers_the_started_event_of_a_subtask_the_caller_already_holds() {
    // The callee the entry gate held: the caller was given the entry
    // when the lower returned, and the callee's parameters were
    // lifted later. The start is what the caller is told about, and
    // the event carries the index and the started state.
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = subtask_in_set(&mut store, &instance, set_index);
    subtask_starts(&mut store, &instance, subtask_index);

    let code = call_u32(&mut store, &instance, "wait", &[Val::U32(set_index)]).await;

    assert_eq!(code, 1, "the subtask event's code");
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        subtask_index,
        "the first payload is the subtask's index in the instance's table"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(4)]).await,
        1,
        "the second payload is the started state"
    );
}

#[wcmp_macros::test]
async fn it_delivers_a_started_that_was_never_taken_as_returned() {
    // The same subtask, which returns before anything takes its
    // event. Delivery reads the record's state at that moment, so the
    // guest is told the call returned and never sees the started
    // state it passed through.
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = subtask_in_set(&mut store, &instance, set_index);
    subtask_starts(&mut store, &instance, subtask_index);
    subtask_returns(&mut store, &instance, subtask_index);

    let code = call_u32(&mut store, &instance, "wait", &[Val::U32(set_index)]).await;

    assert_eq!(code, 1, "the subtask event's code");
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(0)]).await,
        subtask_index,
        "the first payload is still the subtask's index"
    );
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[Val::U32(4)]).await,
        2,
        "the started event that was never taken reads as the returned state"
    );
}

#[wcmp_macros::test]
async fn it_drops_a_subtask_whose_resolution_was_delivered() {
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = subtask_in_set(&mut store, &instance, set_index);
    subtask_starts(&mut store, &instance, subtask_index);
    subtask_returns(&mut store, &instance, subtask_index);
    call_u32(&mut store, &instance, "wait", &[Val::U32(set_index)]).await;

    call(
        &mut store,
        &instance,
        "drop-subtask",
        &[Val::U32(subtask_index)],
    )
    .await
    .expect("the resolution was delivered, so the subtask can be dropped");

    assert_eq!(
        subtask_count(&store),
        0,
        "the record left the store with its entry"
    );
    let message = call_trap(
        &mut store,
        &instance,
        "drop-subtask",
        &[Val::U32(subtask_index)],
    )
    .await;
    assert!(
        message.contains("unknown handle index"),
        "the entry is gone, so the index names nothing: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_a_subtask_whose_resolution_was_not_delivered() {
    // The call returned and its event is waiting, but nothing has
    // taken delivery of it: the handles the call borrowed are still
    // lent out, so the drop is refused.
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = subtask_in_set(&mut store, &instance, set_index);
    subtask_starts(&mut store, &instance, subtask_index);
    subtask_returns(&mut store, &instance, subtask_index);

    let message = call_trap(
        &mut store,
        &instance,
        "drop-subtask",
        &[Val::U32(subtask_index)],
    )
    .await;

    assert!(
        message.contains("cannot drop a subtask which has not yet resolved"),
        "an undelivered resolution refuses the drop: {message}"
    );
    assert_eq!(
        subtask_count(&store),
        1,
        "the refused drop kept the record and its entry"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_a_subtask_that_is_still_running() {
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;
    let subtask_index = subtask_in_set(&mut store, &instance, set_index);
    subtask_starts(&mut store, &instance, subtask_index);

    let message = call_trap(
        &mut store,
        &instance,
        "drop-subtask",
        &[Val::U32(subtask_index)],
    )
    .await;

    assert!(
        message.contains("cannot drop a subtask which has not yet resolved"),
        "a call that has not resolved refuses the drop: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_an_index_that_is_not_a_subtask() {
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;
    let set_index = call_u32(&mut store, &instance, "new-set", &[]).await;

    let message = call_trap(
        &mut store,
        &instance,
        "drop-subtask",
        &[Val::U32(set_index)],
    )
    .await;

    assert!(
        message.contains("is not a subtask"),
        "a waitable set is not a subtask: {message}"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_an_index_that_names_no_entry() {
    let (mut store, instance) = instantiate(SUBTASK_BUILTINS).await;

    let message = call_trap(&mut store, &instance, "drop-subtask", &[Val::U32(7)]).await;

    assert!(
        message.contains("unknown handle index"),
        "an index with no entry names nothing: {message}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_drop_from_a_realloc_with_the_cannot_leave_cause() {
    let (mut store, instance) = instantiate(REALLOC_DROPS_A_SUBTASK).await;

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
        "`subtask.drop` from a realloc must fail with the cannot-leave cause: {message}"
    );
}

#[wcmp_macros::test]
async fn it_takes_a_host_tasks_record_out_of_the_store_with_its_entry() {
    // The whole path: the lower starts the host call and hands the
    // guest an entry, the callback takes delivery of the subtask
    // event, and the drop takes the entry and the record away
    // together. A host call has no task of its own, so the subtask's
    // record is all the store held for it.
    let (mut store, instance) = host_caller().await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(result.first(), Some(&Val::U32(42)), "the host's result");
    assert_eq!(
        subtask_count(&store),
        0,
        "the dropped entry took the host call's record with it"
    );
}

#[wcmp_macros::test]
async fn it_keeps_a_host_tasks_record_in_the_store_while_its_entry_stands() {
    // The same call with the drop left out. The guest still holds the
    // entry, so the record stays: it is the entry that decides, and
    // not the call having finished.
    let (mut store, instance) = host_caller().await;
    call(&mut store, &instance, "select", &[Val::U32(2)])
        .await
        .expect("the guest records that it will not drop the subtask");

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(result.first(), Some(&Val::U32(42)), "the host's result");
    assert_eq!(
        subtask_count(&store),
        1,
        "the record stands while the guest holds its entry"
    );
}

#[wcmp_macros::test]
async fn it_traps_a_drop_of_a_host_subtask_that_has_not_resolved() {
    // The drop moves into `run`, where the host's future has been
    // polled once and is still pending.
    let (mut store, instance) = host_caller().await;
    call(&mut store, &instance, "select", &[Val::U32(1)])
        .await
        .expect("the guest records that it will drop the subtask early");

    let message = match func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
    {
        Err(error) => chain(&error),
        Ok(values) => panic!("run returned {values:?} rather than trapping"),
    };

    assert!(
        message.contains("cannot drop a subtask which has not yet resolved"),
        "a host call still running refuses the drop: {message}"
    );
}
