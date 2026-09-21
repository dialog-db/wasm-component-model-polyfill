//! Baseline tests for the call of a host `async` function through an
//! asynchronous lower.
//!
//! A guest that lowers an import with the `async` option gets control
//! back before the callee returns. The call answers with the status
//! word: the state of the call's subtask in the low four bits, and
//! the subtask's index in the caller's handle table above them.
//!
//! The trampoline lifts the arguments as it always has and then hands
//! the registration's future to the store as a host task. The store
//! polls it once. A future that is ready lowers its result there and
//! then, so the guest sees `RETURNED` and no entry is made for it. A
//! future that is not joins the store's host tasks, its subtask
//! enters the caller's handle table in `STARTED`, and the guest sees
//! that state with the index. A later turn lowers the result through
//! the boundary context of the subtask, resolves the subtask, and
//! fills its event, which `waitable-set.wait` and `waitable-set.poll`
//! deliver as the subtask code, the index, and `RETURNED`.
//!
//! The component below reads all of that back. Its export is lifted
//! `async` with a callback, so a call that has not finished can wait
//! on the subtask where a synchronous export could not: the export
//! returns the wait word naming a set the subtask joined, and the
//! callback receives the event. The index the set is given is what
//! says whether the call left an entry behind — a table hands out
//! index 1 first, so a set that comes back as 1 followed a call that
//! made no entry, and a set that comes back as 2 followed a call
//! whose subtask took the first index.
//!
//! Handles the guest lends for the call go on the subtask's lender
//! list and come back when the resolution is delivered. A borrow lent
//! to a host task therefore stays lent while the call is running *and
//! after its future has completed*, until the guest takes delivery of
//! the subtask event; a guest that drops the owning handle before
//! then traps as PDD014 states.
//!
//! Three more components take the same reading at the lower's other
//! edges. One lowers a *sync-typed* import with the `async` option,
//! which is the pairing that would reach a synchronous registration
//! through this lower: no component can ask for it, because the
//! `async` option may only be used with an `async` function type,
//! and the component is refused where it is read. One carries five
//! `u32` parameters, one flat slot past the four such a lower passes
//! directly, so the whole tuple travels through one pointer into
//! linear memory. One takes delivery through `waitable-set.poll`
//! rather than `waitable-set.wait`, and imports no wait at all, so
//! the triple it reads back can only have come through the poll.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use wasm_component_model_polyfill::{
    Accessor, Component, Engine, Error, Func, FunctionParameter, FunctionType, HostResource,
    Instance, Linker, PrimitiveType, ResourceType, Store, Val, ValueType,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component that lowers an async-typed import with the `async`
/// option and reads the whole of the call back.
///
/// `run` is lifted `async` with a callback. It calls the import with
/// the return area at address 0 and records the status word. It then
/// creates a waitable set, whose index says whether the call left an
/// entry in the handle table. A call that returned gives the guest's
/// own result back at once; a call that started joins its subtask to
/// the set and waits on it, and the callback records the event it is
/// given before returning the result the lowering wrote.
const CALLS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
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
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $status (mut i32) (i32.const -1))
        (global $set (mut i32) (i32.const -1))
        (global $code (mut i32) (i32.const -1))
        (global $first (mut i32) (i32.const -1))
        (global $second (mut i32) (i32.const -1))
        (global $runs (mut i32) (i32.const 0))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (global.set $status (local.get $status))
          (global.set $set (call $set-new))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (global.get $set))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (global.set $code (local.get 0))
          (global.set $first (local.get 1))
          (global.set $second (local.get 2))
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0))
        (func (export "status") (result i32) (global.get $status))
        (func (export "set") (result i32) (global.get $set))
        (func (export "code") (result i32) (global.get $code))
        (func (export "first") (result i32) (global.get $first))
        (func (export "second") (result i32) (global.get $second))
        (func (export "runs") (result i32) (global.get $runs)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback"))))
      (func (export "status") (result u32) (canon lift (core func $m "status")))
      (func (export "set") (result u32) (canon lift (core func $m "set")))
      (func (export "code") (result u32) (canon lift (core func $m "code")))
      (func (export "first") (result u32) (canon lift (core func $m "first")))
      (func (export "second") (result u32) (canon lift (core func $m "second")))
      (func (export "runs") (result u32) (canon lift (core func $m "runs"))))
    "#
);

/// A component that lends a borrow of a host resource to a host
/// `async` function it calls through an asynchronous lower.
///
/// `start` mints an owning handle, calls the import with a borrow of
/// it, and joins the subtask it is given to a set of its own.
/// `drop-now` drops the owning handle where it stands, which traps
/// while the borrow is still lent. `finish` waits on the set, so the
/// event is delivered and the lend is undone, and drops the handle
/// from its callback.
const LENDS_A_BORROW_TO_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "host" (instance $host
        (export "thing" (type $t (sub resource)))
        (export "[constructor]thing" (func (param "rep" u32) (result (own $t))))
        (export "hold" (func async (param "it" (borrow $t))))))
      (alias export $host "thing" (type $t))
      (alias export $host "[constructor]thing" (func $make))
      (alias export $host "hold" (func $hold))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $make (canon lower (func $make)))
      (core func $hold
        (canon lower (func $hold) async (memory (core memory $libc "memory"))))
      (core func $drop-thing (canon resource.drop $t))
      (core func $task-return (canon task.return))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "" "make" (func $make (param i32) (result i32)))
        (import "" "hold" (func $hold (param i32) (result i32)))
        (import "" "drop-thing" (func $drop-thing (param i32)))
        (import "" "task.return" (func $task-return))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $handle (mut i32) (i32.const 0))
        (global $set (mut i32) (i32.const 0))
        (func (export "start") (result i32)
          (local $status i32)
          (global.set $handle (call $make (i32.const 100)))
          (local.set $status (call $hold (global.get $handle)))
          (global.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (global.get $set))
          (local.get $status))
        (func (export "drop-now") (call $drop-thing (global.get $handle)))
        (func (export "finish") (result i32)
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "finish-callback") (param i32 i32 i32) (result i32)
          (call $drop-thing (global.get $handle))
          (call $task-return)
          (i32.const 0)))
      (core instance $m (instantiate $m
        (with "" (instance
          (export "make" (func $make))
          (export "hold" (func $hold))
          (export "drop-thing" (func $drop-thing))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "start") (result u32) (canon lift (core func $m "start")))
      (func (export "drop-now") (canon lift (core func $m "drop-now")))
      (func (export "finish") async
        (canon lift (core func $m "finish") async
          (callback (core func $m "finish-callback")))))
    "#
);

/// A component that lowers a *sync-typed* import with the `async`
/// option, which is not a component anything accepts.
///
/// A sync-typed import is the one an import takes a synchronous
/// registration for, so this is the shape a guest would have to
/// bring to reach a synchronous registration through an asynchronous
/// lower. It is refused where the component is read: the `async`
/// canonical option may only be used with an `async` function type.
///
/// The body is [`CALLS_A_HOST_ASYNC_FUNCTION`]'s, with the `async`
/// effect taken off the import and nothing else changed, so what the
/// refusal answers is that one difference.
const CALLS_A_HOST_SYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $status (mut i32) (i32.const -1))
        (global $set (mut i32) (i32.const -1))
        (global $code (mut i32) (i32.const -1))
        (global $first (mut i32) (i32.const -1))
        (global $second (mut i32) (i32.const -1))
        (global $runs (mut i32) (i32.const 0))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (global.set $status (local.get $status))
          (global.set $set (call $set-new))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (global.get $set))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (global.set $runs (i32.add (global.get $runs) (i32.const 1)))
          (global.set $code (local.get 0))
          (global.set $first (local.get 1))
          (global.set $second (local.get 2))
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0))
        (func (export "status") (result i32) (global.get $status))
        (func (export "set") (result i32) (global.get $set))
        (func (export "code") (result i32) (global.get $code))
        (func (export "first") (result i32) (global.get $first))
        (func (export "second") (result i32) (global.get $second))
        (func (export "runs") (result i32) (global.get $runs)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback"))))
      (func (export "status") (result u32) (canon lift (core func $m "status")))
      (func (export "set") (result u32) (canon lift (core func $m "set")))
      (func (export "code") (result u32) (canon lift (core func $m "code")))
      (func (export "first") (result u32) (canon lift (core func $m "first")))
      (func (export "second") (result u32) (canon lift (core func $m "second")))
      (func (export "runs") (result u32) (canon lift (core func $m "runs"))))
    "#
);

/// A component whose asynchronous lower carries five `u32`
/// parameters: one flat slot more than such a lower passes directly.
///
/// The whole tuple therefore travels through one pointer into linear
/// memory, laid out as the canonical ABI lays a record of the five
/// types out — five `u32`s four bytes apart. The guest writes them
/// at address 16 and passes that address, and the return area the
/// second argument names stays at address 0.
///
/// The rest is [`CALLS_A_HOST_ASYNC_FUNCTION`]'s shape: the export
/// is lifted `async` with a callback, so the guest can wait on the
/// subtask a pending future leaves it.
const SPILLS_THE_PARAMETERS_OF_AN_ASYNCHRONOUS_LOWER: &[u8] = component!(
    r#"
    (component
      (import "sum" (func $sum async
        (param "a" u32) (param "b" u32) (param "c" u32)
        (param "d" u32) (param "e" u32)
        (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $sum) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result u32)))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "sum" (func $sum (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (global $status (mut i32) (i32.const -1))
        (global $set (mut i32) (i32.const -1))
        (global $code (mut i32) (i32.const -1))
        (global $first (mut i32) (i32.const -1))
        (global $second (mut i32) (i32.const -1))
        (func (export "run") (param i32) (result i32)
          (local $status i32)
          (i32.store (i32.const 16) (local.get 0))
          (i32.store (i32.const 20) (i32.add (local.get 0) (i32.const 1)))
          (i32.store (i32.const 24) (i32.add (local.get 0) (i32.const 2)))
          (i32.store (i32.const 28) (i32.add (local.get 0) (i32.const 3)))
          (i32.store (i32.const 32) (i32.add (local.get 0) (i32.const 4)))
          (local.set $status (call $sum (i32.const 16) (i32.const 0)))
          (global.set $status (local.get $status))
          (global.set $set (call $set-new))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $task-return (i32.load (i32.const 0)))
              (return (i32.const 0))))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (global.get $set))
          (i32.or (i32.shl (global.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (global.set $code (local.get 0))
          (global.set $first (local.get 1))
          (global.set $second (local.get 2))
          (call $task-return (i32.load (i32.const 0)))
          (i32.const 0))
        (func (export "status") (result i32) (global.get $status))
        (func (export "set") (result i32) (global.get $set))
        (func (export "code") (result i32) (global.get $code))
        (func (export "first") (result i32) (global.get $first))
        (func (export "second") (result i32) (global.get $second)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "sum" (func $lowered))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback"))))
      (func (export "status") (result u32) (canon lift (core func $m "status")))
      (func (export "set") (result u32) (canon lift (core func $m "set")))
      (func (export "code") (result u32) (canon lift (core func $m "code")))
      (func (export "first") (result u32) (canon lift (core func $m "first")))
      (func (export "second") (result u32) (canon lift (core func $m "second"))))
    "#
);

/// A component that takes delivery of a host subtask's event through
/// `waitable-set.poll` rather than `waitable-set.wait`.
///
/// Nothing here waits. `start` calls the import through an
/// asynchronous lower, creates a set, joins the subtask it was given
/// to it, and polls the set once before it returns — where the call
/// cannot yet have resolved, so that poll answers the none code and
/// writes nothing. `poll` polls the same set again, and the guest
/// reads the two payload words out of memory at address 8. The
/// component imports no `waitable-set.wait` at all, so an event it
/// reads back can only have come through the poll.
///
/// Both exports are synchronous, which a poll allows: it never
/// blocks.
const POLLS_A_HOST_SUBTASK: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $lowered
        (canon lower (func $answer) async (memory (core memory $libc "memory"))))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core func $poll (canon waitable-set.poll (memory (core memory $libc "memory"))))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "answer" (func $answer (param i32 i32) (result i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (import "" "waitable-set.poll" (func $poll (param i32 i32) (result i32)))
        (global $set (mut i32) (i32.const 0))
        (global $early (mut i32) (i32.const -1))
        (func (export "start") (param i32) (result i32)
          (local $status i32)
          (local.set $status (call $answer (local.get 0) (i32.const 0)))
          (global.set $set (call $set-new))
          (call $join
            (i32.shr_u (local.get $status) (i32.const 4))
            (global.get $set))
          (global.set $early (call $poll (global.get $set) (i32.const 8)))
          (local.get $status))
        (func (export "poll") (result i32)
          (call $poll (global.get $set) (i32.const 8)))
        (func (export "set") (result i32) (global.get $set))
        (func (export "early") (result i32) (global.get $early))
        (func (export "first") (result i32) (i32.load (i32.const 8)))
        (func (export "second") (result i32) (i32.load (i32.const 12)))
        (func (export "answer") (result i32) (i32.load (i32.const 0))))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "answer" (func $lowered))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))
          (export "waitable-set.poll" (func $poll))))))
      (func (export "start") (param "x" u32) (result u32)
        (canon lift (core func $m "start")))
      (func (export "poll") (result u32) (canon lift (core func $m "poll")))
      (func (export "set") (result u32) (canon lift (core func $m "set")))
      (func (export "early") (result u32) (canon lift (core func $m "early")))
      (func (export "first") (result u32) (canon lift (core func $m "first")))
      (func (export "second") (result u32) (canon lift (core func $m "second")))
      (func (export "answer") (result u32) (canon lift (core func $m "answer"))))
    "#
);

/// A future that is pending the first time it is polled and ready
/// afterwards, answering with `value`. It wakes the waker it was
/// polled with before it parks, which is what a future waiting on a
/// timer or a promise has its host do for it.
///
/// One poll apart is all a call needs to be a subtask: the
/// trampoline's own poll is the first, so the call starts, and the
/// next turn's poll completes it.
struct PendingOnce<V> {
    polled: bool,
    value: Option<V>,
}

impl<V> PendingOnce<V> {
    fn new(value: V) -> Self {
        Self {
            polled: false,
            value: Some(value),
        }
    }
}

impl<V: Unpin> Future for PendingOnce<V> {
    type Output = Result<V, Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.polled {
            let value = this.value.take().expect("the future is polled once ready");
            return Poll::Ready(Ok(value));
        }
        this.polled = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

/// The declared type of the `answer` import: one `u32` in, one `u32`
/// out, carrying the `async` effect the link rule reads.
fn answer_type() -> FunctionType {
    FunctionType {
        parameters: vec![FunctionParameter {
            name: "x".to_owned(),
            ty: ValueType::Primitive(PrimitiveType::U32),
        }],
        result: Some(ValueType::Primitive(PrimitiveType::U32)),
        async_: true,
    }
}

/// The declared type of the `sum` import: five `u32`s in, one out.
/// Five flat slots are one past the four an asynchronous lower
/// passes directly, so the tuple spills.
fn sum_type() -> FunctionType {
    FunctionType {
        parameters: ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|name| FunctionParameter {
                name: name.to_owned(),
                ty: ValueType::Primitive(PrimitiveType::U32),
            })
            .collect(),
        result: Some(ValueType::Primitive(PrimitiveType::U32)),
        async_: true,
    }
}

/// Instantiate [`CALLS_A_HOST_ASYNC_FUNCTION`] with `register`
/// registering the `answer` import.
async fn caller<F>(register: F) -> (Store<()>, Instance)
where
    F: FnOnce(&mut Linker<()>),
{
    instantiate(CALLS_A_HOST_ASYNC_FUNCTION, register).await
}

/// Instantiate `binary` in a fresh store, with `register` making
/// every host registration its imports name.
async fn instantiate<F>(binary: &[u8], register: F) -> (Store<()>, Instance)
where
    F: FnOnce(&mut Linker<()>),
{
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, binary)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    register(&mut linker);
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

/// Call the named `u32`-returning export of the instance.
async fn read(store: &mut Store<()>, instance: &Instance, name: &str) -> u32 {
    match func(instance, name)
        .call(store, &[])
        .await
        .unwrap_or_else(|err| panic!("call `{name}`: {err}"))
        .first()
    {
        Some(Val::U32(value)) => *value,
        other => panic!("`{name}` answered with {other:?}"),
    }
}

/// Every message in an error's source chain, joined so a cause the
/// substrate wrapped can be matched wherever it put it.
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
    out
}

/// The four facts a call of `run` leaves behind: the status word, the
/// index the waitable set was given, and the event the callback
/// received as the subtask code with its two payloads.
async fn recorded(store: &mut Store<()>, instance: &Instance) -> (u32, u32, (u32, u32, u32)) {
    let status = read(store, instance, "status").await;
    let set = read(store, instance, "set").await;
    let code = read(store, instance, "code").await;
    let first = read(store, instance, "first").await;
    let second = read(store, instance, "second").await;
    (status, set, (code, first, second))
}

/// The status word of a call that returned before the lower did:
/// `RETURNED` with no index above it.
const RETURNED: u32 = 2;

/// The status word of a call that is still running, waited on through
/// the handle-table entry at index 1: `STARTED` with that index above
/// it.
const STARTED_AT_ONE: u32 = 1 | (1 << 4);

/// The status word of a call that is still running, waited on through
/// the entry at index 2. The component that lends a borrow reaches
/// this one: the owning handle it mints first takes index 1 of the
/// instance's table, which subtasks, waitable sets and resources all
/// share.
const STARTED_AT_TWO: u32 = 1 | (2 << 4);

/// The event a resolved subtask at index 1 delivers: the subtask
/// code, the index, and the returned state.
const SUBTASK_RETURNED_AT_ONE: (u32, u32, u32) = (1, 1, 2);

/// The event slots of a callback that never ran, which each global
/// holds from instantiation.
const NO_EVENT: (u32, u32, u32) = (u32::MAX, u32::MAX, u32::MAX);

/// The code a `waitable-set.poll` of a set that holds no event
/// answers with: the none event, which carries no payloads.
const NONE_EVENT_CODE: u32 = 0;

#[wcmp_macros::test]
async fn it_returns_at_once_from_a_typed_registration_whose_future_is_ready() {
    // The future resolves on its first poll, so the trampoline lowers
    // its result where it stands and the guest is told the call
    // returned. Nothing is left to wait on: the set the guest creates
    // next comes back as index 1, which is the first index a table
    // hands out, so the call made no entry of its own.
    let (mut store, instance) = caller(|linker| {
        linker.root().func_wrap_concurrent(
            "answer",
            |_accessor: &Accessor<()>, (x,): (u32,)| async move { Ok(x * 2) },
        );
    })
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(result.first(), Some(&Val::U32(42)), "the host's result");
    assert_eq!(
        recorded(&mut store, &instance).await,
        (RETURNED, 1, NO_EVENT),
        "the guest saw the returned status with no index, the set took the \
         first index the table hands out, and no subtask event was delivered"
    );
}

#[wcmp_macros::test]
async fn it_returns_at_once_from_an_untyped_registration_whose_future_is_ready() {
    // The same call through the untyped entry, whose future answers
    // with the value vector itself.
    let (mut store, instance) = caller(|linker| {
        linker.root().func_new_concurrent(
            "answer",
            answer_type(),
            |_accessor: &Accessor<()>, args: Vec<Val>| async move {
                let Some(Val::U32(x)) = args.first() else {
                    panic!("`answer` was given {args:?}");
                };
                Ok(vec![Val::U32(x * 2)])
            },
        );
    })
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(result.first(), Some(&Val::U32(42)), "the host's result");
    assert_eq!(
        recorded(&mut store, &instance).await,
        (RETURNED, 1, NO_EVENT),
        "the untyped entry's ready future reads back exactly as the typed \
         entry's does"
    );
}

#[wcmp_macros::test]
async fn it_refuses_an_asynchronous_lower_of_a_sync_typed_import() {
    // A synchronous registration reached through an asynchronous
    // lower is the pairing that would answer the status word without
    // ever building a host task. No component can ask for it: the
    // `async` canonical option may only be used with an `async`
    // function type, and a sync-typed import lowered with it is
    // refused where the component is read, before any registration
    // is consulted. The rule is the Explainer's, and the conformance
    // corpus carries the same three cases as `assert_invalid`.
    //
    // So the guest reaches a synchronous registration through a
    // synchronous lower and nothing else, and the trampoline's
    // returned-at-once path is a concurrent registration's ready
    // future — the two tests above it.
    let engine = Engine::new().expect("engine");
    let error = Component::new(&engine, CALLS_A_HOST_SYNC_FUNCTION)
        .await
        .expect_err("a sync-typed import cannot be lowered with the `async` option");

    assert!(
        chain(&error).contains("the `async` canonical option requires an async function type"),
        "expected the validator's refusal of the `async` option on a sync-typed \
         function, got {error:?}"
    );
}

#[wcmp_macros::test]
async fn it_starts_a_subtask_for_a_typed_registration_whose_future_is_pending() {
    // The future is pending on its first poll, so the call joins the
    // store's host tasks and the guest is told the call started, at
    // the index the subtask took. The set the guest creates next is
    // index 2, which is what says the subtask holds index 1. A later
    // turn lowers the result and fills the subtask event, and the
    // callback the wait resumes receives it as the subtask code, that
    // index, and the returned state.
    let (mut store, instance) = caller(|linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                PendingOnce::new(x * 2)
            });
    })
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the result the later turn lowered reached the guest"
    );
    assert_eq!(
        recorded(&mut store, &instance).await,
        (STARTED_AT_ONE, 2, SUBTASK_RETURNED_AT_ONE),
        "the guest saw the started status with the subtask's index, the set \
         took the index after it, and the event delivered the subtask code \
         with that index and the returned state"
    );
    assert_eq!(
        read(&mut store, &instance, "runs").await,
        1,
        "the callback ran once, for the one event the subtask delivered"
    );
}

#[wcmp_macros::test]
async fn it_starts_a_subtask_for_an_untyped_registration_whose_future_is_pending() {
    // The same call through the untyped entry.
    let (mut store, instance) = caller(|linker| {
        linker.root().func_new_concurrent(
            "answer",
            answer_type(),
            |_accessor: &Accessor<()>, args: Vec<Val>| {
                let Some(Val::U32(x)) = args.first() else {
                    panic!("`answer` was given {args:?}");
                };
                PendingOnce::new(vec![Val::U32(x * 2)])
            },
        );
    })
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the result the later turn lowered reached the guest"
    );
    assert_eq!(
        recorded(&mut store, &instance).await,
        (STARTED_AT_ONE, 2, SUBTASK_RETURNED_AT_ONE),
        "the untyped entry's pending future reads back exactly as the typed \
         entry's does"
    );
}

#[wcmp_macros::test]
async fn it_lifts_the_spilled_parameters_of_an_asynchronous_lower_through_the_pointer() {
    // Five `u32` parameters are five flat slots, one past the four
    // an asynchronous lower passes directly, so the whole tuple
    // travels through one pointer: the guest laid the five out at
    // address 16, four bytes apart, and passed that address with the
    // return area as the lower's only two arguments. The future is
    // pending once, so the call is a subtask and the result crosses
    // in a later turn — through the return area the lower read past
    // the tuple's pointer.
    let (mut store, instance) =
        instantiate(SPILLS_THE_PARAMETERS_OF_AN_ASYNCHRONOUS_LOWER, |linker| {
            linker.root().func_new_concurrent(
                "sum",
                sum_type(),
                |_accessor: &Accessor<()>, args: Vec<Val>| {
                    // A sum reads the same for any permutation, so
                    // the tuple is checked position by position: the
                    // five values have to arrive in the order the
                    // guest laid them out.
                    assert_eq!(
                        args,
                        (10..15).map(Val::U32).collect::<Vec<Val>>(),
                        "`sum` was given {args:?}"
                    );
                    let total = args
                        .iter()
                        .map(|arg| match arg {
                            Val::U32(value) => *value,
                            other => panic!("`sum` was given {other:?}"),
                        })
                        .sum::<u32>();
                    PendingOnce::new(vec![Val::U32(total)])
                },
            );
        })
        .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(10)])
        .await
        .expect("call run");

    assert_eq!(
        result.first(),
        Some(&Val::U32(60)),
        "the five parameters reached the host in order — 10 through 14, \
         checked position by position in the registration — and their sum \
         crossed back through the return area"
    );
    assert_eq!(
        recorded(&mut store, &instance).await,
        (STARTED_AT_ONE, 2, SUBTASK_RETURNED_AT_ONE),
        "a spilled parameter tuple leaves the rest of the lower unchanged: \
         the started status with the subtask's index, the set after it, and \
         the subtask event the later turn filled"
    );
}

#[wcmp_macros::test]
async fn it_delivers_a_host_subtask_event_through_a_poll_of_the_waitable_set() {
    // The guest never waits. It calls the import through an
    // asynchronous lower, joins the subtask it is given to a set of
    // its own, and reads the set with `waitable-set.poll`, which
    // never blocks: the poll it makes before returning finds nothing
    // — the call cannot have resolved with the guest still on the
    // stack — and a later poll takes delivery of the event. The
    // triple it reads back is the one `waitable-set.wait` delivers
    // to the tests above: the subtask code, the subtask's index in
    // the caller's handle table, and the returned state.
    let (mut store, instance) = instantiate(POLLS_A_HOST_SUBTASK, |linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                PendingOnce::new(x * 2)
            });
    })
    .await;

    let started = func(&instance, "start")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call start");

    assert_eq!(
        started.first(),
        Some(&Val::U32(STARTED_AT_ONE)),
        "the call started, at the index the subtask took"
    );
    assert_eq!(
        read(&mut store, &instance, "early").await,
        NONE_EVENT_CODE,
        "the poll the guest made before it returned found no event, which is \
         what a poll that does not block answers with"
    );

    // The event is filled by the turn that lowers the result, which
    // is a later turn than the one the call started in. Polling
    // until it lands is what a guest that will not block does.
    let mut code = NONE_EVENT_CODE;
    for _ in 0..8 {
        code = read(&mut store, &instance, "poll").await;
        if code != NONE_EVENT_CODE {
            break;
        }
    }
    let first = read(&mut store, &instance, "first").await;
    let second = read(&mut store, &instance, "second").await;

    assert_eq!(
        (code, first, second),
        SUBTASK_RETURNED_AT_ONE,
        "the poll delivered the subtask code with the subtask's index and the \
         returned state, which is the triple a wait delivers"
    );
    assert_eq!(
        read(&mut store, &instance, "answer").await,
        42,
        "the later turn lowered the host's result into the return area the \
         lower passed"
    );
    assert_eq!(
        read(&mut store, &instance, "poll").await,
        NONE_EVENT_CODE,
        "the event was taken by the poll that delivered it, so the set holds \
         none afterwards"
    );
}

#[wcmp_macros::test]
async fn it_fails_the_call_when_an_untyped_future_answers_with_the_wrong_arity() {
    // The declared type has a result, so the future must complete
    // with one value. A future that completes with none is the
    // mistake the untyped synchronous path reports as a host value
    // that does not match the declared type, seen a call later.
    let (mut store, instance) = caller(|linker| {
        linker.root().func_new_concurrent(
            "answer",
            answer_type(),
            |_accessor: &Accessor<()>, _args: Vec<Val>| async move { Ok(Vec::new()) },
        );
    })
    .await;

    let err = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect_err("the call fails");
    assert!(
        chain(&err).contains("host value variant does not match declared value type"),
        "expected the untyped path's host-value mismatch, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_fails_an_untyped_synchronous_call_with_the_same_mismatch() {
    // The same mistake through the untyped synchronous entry, which
    // is what the concurrent entry's arity check is held to: a host
    // value the declared type does not describe.
    const SYNCHRONOUS_CALLER: &[u8] = component!(
        r#"
        (component
          (import "answer" (func $answer (param "x" u32) (result u32)))
          (core func $lowered (canon lower (func $answer)))
          (core module $m
            (import "" "answer" (func $answer (param i32) (result i32)))
            (func (export "run") (param i32) (result i32) (call $answer (local.get 0))))
          (core instance $i (instantiate $m
            (with "" (instance (export "answer" (func $lowered))))))
          (func (export "run") (param "x" u32) (result u32)
            (canon lift (core func $i "run"))))
        "#
    );
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, SYNCHRONOUS_CALLER)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    let mut declared = answer_type();
    declared.async_ = false;
    linker
        .root()
        .func_new("answer", declared, |_call, _args, results| {
            results[0] = Val::Bool(false);
            Ok(())
        });
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let err = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect_err("the call fails");
    assert!(
        chain(&err).contains("host value variant does not match declared value type"),
        "expected the untyped synchronous path's host-value mismatch, got {err:?}"
    );
}

/// Instantiate [`LENDS_A_BORROW_TO_A_HOST_ASYNC_FUNCTION`] with the
/// `host` instance its import names: a resource, a constructor that
/// mints a handle for it, and a host `async` function that takes a
/// borrow and whose future is pending on its first poll.
async fn lender() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LENDS_A_BORROW_TO_A_HOST_ASYNC_FUNCTION)
        .await
        .expect("component parses");
    let mut linker: Linker<()> = Linker::new(&engine);
    {
        let mut root = linker.root();
        let mut host = root.instance("host");
        let thing = host.resource_with("thing", HostResource::new(|_, _| Ok(())));
        host.func_new(
            "[constructor]thing",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "rep".to_owned(),
                    ty: ValueType::Primitive(PrimitiveType::U32),
                }],
                result: Some(ValueType::Own(ResourceType::new("thing"))),
                async_: false,
            },
            move |call, args, results| {
                let Some(Val::U32(rep)) = args.first() else {
                    panic!("`[constructor]thing` was given {args:?}");
                };
                results[0] = Val::Own(call.resource_new(thing, *rep)?);
                Ok(())
            },
        );
        host.func_new_concurrent(
            "hold",
            FunctionType {
                parameters: vec![FunctionParameter {
                    name: "it".to_owned(),
                    ty: ValueType::Borrow(ResourceType::new("thing")),
                }],
                result: None,
                async_: true,
            },
            |_accessor: &Accessor<()>, _args: Vec<Val>| PendingOnce::new(Vec::<Val>::new()),
        );
    }

    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_keeps_a_borrow_lent_to_a_host_task_until_the_event_is_delivered() {
    // The guest lends a borrow of its owning handle for the call and
    // is told the call started. By the time it calls `drop-now` the
    // future has completed — the turn that ran the export polled it
    // once more — but the guest has not taken delivery of the subtask
    // event, so the lend stands and the drop of the owning handle
    // traps.
    let (mut store, instance) = lender().await;

    assert_eq!(
        read(&mut store, &instance, "start").await,
        STARTED_AT_TWO,
        "the call started, at the index the subtask took"
    );

    let err = func(&instance, "drop-now")
        .call(&mut store, &[])
        .await
        .expect_err("the owning handle cannot be dropped while it is lent");
    assert!(
        chain(&err).contains("cannot remove owned resource while borrowed"),
        "expected the lent-handle trap, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_returns_a_borrow_lent_to_a_host_task_when_the_event_is_delivered() {
    // The same call, waited on rather than abandoned: the wait
    // delivers the subtask event, which gives back every handle the
    // call borrowed, and the callback drops the owning handle where
    // the previous test's `drop-now` trapped.
    let (mut store, instance) = lender().await;

    assert_eq!(
        read(&mut store, &instance, "start").await,
        STARTED_AT_TWO,
        "the call started, at the index the subtask took"
    );

    func(&instance, "finish")
        .call(&mut store, &[])
        .await
        .expect("the wait delivers the subtask event and the drop succeeds");
}

/// The browser's proof that a host `async` function may await a
/// JavaScript promise: the registration's future resolves a promise
/// and answers with its value, and the guest receives that value
/// through an asynchronous lower.
///
/// Nothing of the promise is ready when the trampoline polls the
/// future, so the call starts a subtask and the guest waits on it.
/// The page's microtask queue resolves the promise, the wake reaches
/// the driver, and a later turn lowers the value into the guest.
#[cfg(target_arch = "wasm32")]
#[wcmp_macros::test]
async fn it_awaits_a_javascript_promise_and_gives_the_guest_its_value() {
    let (mut store, instance) = caller(|linker| {
        linker
            .root()
            .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
                let promise =
                    js_sys::Promise::resolve(&wasm_bindgen::JsValue::from_f64(f64::from(x * 2)));
                async move {
                    let resolved = wasm_bindgen_futures::JsFuture::from(promise)
                        .await
                        .expect("the promise resolves");
                    Ok(resolved.as_f64().expect("the promise's value") as u32)
                }
            });
    })
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call run");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the promise's value reached the guest"
    );
    assert_eq!(
        recorded(&mut store, &instance).await,
        (STARTED_AT_ONE, 2, SUBTASK_RETURNED_AT_ONE),
        "the promise was not ready when the trampoline polled the future, so \
         the call started a subtask the guest waited on"
    );
}
