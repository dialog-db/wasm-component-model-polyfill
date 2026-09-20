//! Baseline tests for the call of a host `async` function through a
//! synchronous lower.
//!
//! A guest that lowers an import without the `async` option expects
//! the result when the call returns, so a host function whose future
//! is not ready at once has to block the guest thread where it
//! stands. The block runs through the suspend seam on every target,
//! and the blocked call's own future is polled at every check of the
//! seam's condition rather than from the store, because the store
//! does not hold it: the call it belongs to is still on the guest's
//! stack.
//!
//! A future that is pending once and then ready — which is what a
//! future that yields once comes to — therefore resolves inside the
//! nested turn the block runs, and the call returns its result
//! through the flat results of the synchronous lower. Only a future
//! that stays pending fails the call, and what it fails with is the
//! cause the seam selects: the stack-switch cause when the caller is
//! a task that is allowed to block, and the cannot-block cause when
//! some sync-typed call of the store has yet to return.
//!
//! The call is over by the time the lower returns, so the subtask's
//! resolution is delivered there: the handles the guest lent for the
//! call go back, and a handle the guest drops on the next instruction
//! is no longer lent. That is the difference from the asynchronous
//! lower of `baseline_async_lower.rs`, where the lend stands until
//! the guest takes delivery of the subtask event.

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

/// A component whose sync-typed export calls an async-typed import
/// through a synchronous lower and hands the result straight back.
///
/// The export is lifted without `async`, so the host call into it is
/// a sync-typed call: its instance carries may-not-suspend for the
/// length of the call, which is what decides the cause when the
/// import's future never resolves.
const A_SYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (func (export "run") (param i32) (result i32)
          (call $answer (local.get 0))))
      (core instance $m (instantiate $m
        (with "" (instance (export "answer" (func $lowered))))))
      (func (export "run") (param "x" u32) (result u32)
        (canon lift (core func $m "run"))))
    "#
);

/// A component whose async-typed export calls the same import through
/// the same synchronous lower.
///
/// The export is lifted `async` with a callback, so its task is one
/// the reference allows to block: no instance carries may-not-suspend
/// while it runs. Its core function calls the import synchronously
/// all the same — the two axes move separately — and returns the
/// result through `task.return`, which a call that traps never
/// reaches.
const AN_ASYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION: &[u8] = component!(
    r#"
    (component
      (import "answer" (func $answer async (param "x" u32) (result u32)))
      (core func $lowered (canon lower (func $answer)))
      (core func $task-return (canon task.return (result u32)))
      (core module $m
        (import "" "answer" (func $answer (param i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32)))
        (func (export "run") (param i32) (result i32)
          (call $task-return (call $answer (local.get 0)))
          (i32.const 0))
        (func (export "run-callback") (param i32 i32 i32) (result i32)
          (unreachable)))
      (core instance $m (instantiate $m
        (with "" (instance
          (export "answer" (func $lowered))
          (export "task.return" (func $task-return))))))
      (func (export "run") async (param "x" u32) (result u32)
        (canon lift (core func $m "run") async
          (callback (core func $m "run-callback")))))
    "#
);

/// A component that lends a borrow of a host resource to a host
/// `async` function it calls through a synchronous lower, and drops
/// the owning handle on the next instruction.
///
/// The drop stands or traps on whether the lend came back. It comes
/// back when the call's subtask resolves, and a synchronous lower
/// resolves it before it returns, so `run` completes and answers with
/// the rep it minted the handle for.
const LENDS_A_BORROW_THROUGH_A_SYNCHRONOUS_LOWER: &[u8] = component!(
    r#"
    (component
      (import "host" (instance $host
        (export "thing" (type $t (sub resource)))
        (export "[constructor]thing" (func (param "rep" u32) (result (own $t))))
        (export "hold" (func async (param "it" (borrow $t))))))
      (alias export $host "thing" (type $t))
      (alias export $host "[constructor]thing" (func $make))
      (alias export $host "hold" (func $hold))
      (core func $make (canon lower (func $make)))
      (core func $hold (canon lower (func $hold)))
      (core func $drop-thing (canon resource.drop $t))
      (core module $m
        (import "" "make" (func $make (param i32) (result i32)))
        (import "" "hold" (func $hold (param i32)))
        (import "" "drop-thing" (func $drop-thing (param i32)))
        (func (export "run") (result i32)
          (local $handle i32)
          (local.set $handle (call $make (i32.const 100)))
          (call $hold (local.get $handle))
          (call $drop-thing (local.get $handle))
          (i32.const 100)))
      (core instance $m (instantiate $m
        (with "" (instance
          (export "make" (func $make))
          (export "hold" (func $hold))
          (export "drop-thing" (func $drop-thing))))))
      (func (export "run") (result u32) (canon lift (core func $m "run"))))
    "#
);

/// A future that is pending the first time it is polled and ready
/// afterwards, answering with `value`. It wakes the waker it was
/// polled with before it parks, which is what a future waiting on a
/// timer or a promise has its host do for it.
///
/// Through a synchronous lower the second poll is the one the block's
/// condition makes after the first nested turn found nothing to run.
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

/// Instantiate `bytes` with `register` registering the `answer`
/// import.
async fn caller<F>(bytes: &[u8], register: F) -> (Store<()>, Instance)
where
    F: FnOnce(&mut Linker<()>),
{
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
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

/// Register `answer` with a typed concurrent entry whose future is
/// pending once and then answers with twice its argument.
fn pending_once_answer(linker: &mut Linker<()>) {
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (x,): (u32,)| {
            PendingOnce::new(x * 2)
        });
}

/// Register `answer` with a typed concurrent entry whose future never
/// resolves, which is what `never-return` is in Wasmtime's spectest.
fn never_answers(linker: &mut Linker<()>) {
    linker
        .root()
        .func_wrap_concurrent("answer", |_accessor: &Accessor<()>, (_x,): (u32,)| {
            core::future::pending::<Result<u32, Error>>()
        });
}

#[wcmp_macros::test]
async fn it_returns_the_result_of_a_future_that_is_pending_once() {
    // The future is not ready when the trampoline polls it, so the
    // call blocks the guest thread. The block's condition polls the
    // future again after the first nested turn found nothing to run,
    // the future is ready then, and the call returns its result
    // through the lower's flat results — with no subtask left for the
    // guest to wait on, since the guest never got control back.
    let (mut store, instance) = caller(
        A_SYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION,
        pending_once_answer,
    )
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the blocked call resolves inside the nested turn");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the host's result crossed as the synchronous lower returned"
    );
}

#[wcmp_macros::test]
async fn it_returns_the_result_of_a_pending_once_future_to_an_async_typed_task() {
    // The same call from a task that is allowed to block. The rule is
    // the same: the block polls the call's own future from its
    // condition, and a future ready on the second poll resolves
    // there.
    let (mut store, instance) = caller(
        AN_ASYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION,
        pending_once_answer,
    )
    .await;

    let result = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("the blocked call resolves inside the nested turn");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the core function reached `task.return` with the host's result"
    );
}

#[wcmp_macros::test]
async fn it_returns_the_result_of_an_untyped_registration_that_is_pending_once() {
    // The same call through the untyped concurrent entry, whose
    // future answers with the value vector itself.
    let (mut store, instance) = caller(A_SYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION, |linker| {
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
        .expect("the blocked call resolves inside the nested turn");

    assert_eq!(
        result.first(),
        Some(&Val::U32(42)),
        "the untyped entry's pending future reads back exactly as the typed \
         entry's does"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_future_that_stays_pending_with_the_stack_switch_cause() {
    // The caller is an async-typed task, so no instance of the store
    // carries may-not-suspend. The nested turns find nothing to run
    // and the store goes idle with the call's own future still
    // pending, which is the state the reference permits a block in
    // and only the target cannot serve.
    let (mut store, instance) = caller(
        AN_ASYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION,
        never_answers,
    )
    .await;

    let err = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect_err("a future that never resolves cannot be blocked on to the end");

    assert!(
        chain(&err).contains("blocking here requires a stack switch"),
        "expected the stack-switch cause, got {err:?}"
    );
}

#[wcmp_macros::test]
async fn it_fails_a_future_that_stays_pending_with_the_cannot_block_cause() {
    // The caller is a sync-typed task that has not returned, so its
    // instance carries may-not-suspend: the block gives way to the
    // ready work of that instance alone, polls no host task, and
    // fails with the caller's rule rather than the target's.
    let (mut store, instance) =
        caller(A_SYNC_TYPED_TASK_CALLS_A_HOST_ASYNC_FUNCTION, never_answers).await;

    let err = func(&instance, "run")
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect_err("a future that never resolves cannot be blocked on to the end");

    assert!(
        chain(&err).contains("cannot block a synchronous task before returning"),
        "expected the cannot-block cause, got {err:?}"
    );
}

/// Instantiate [`LENDS_A_BORROW_THROUGH_A_SYNCHRONOUS_LOWER`] with
/// the `host` instance its import names: a resource, a constructor
/// that mints a handle for it, and a host `async` function that takes
/// a borrow and whose future is pending on its first poll.
async fn lender() -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, LENDS_A_BORROW_THROUGH_A_SYNCHRONOUS_LOWER)
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
async fn it_releases_a_handle_lent_for_the_call_when_the_lower_returns() {
    // The guest lends a borrow of its owning handle for the call and
    // drops the owning handle on the instruction after it. The drop
    // traps while the borrow is still lent, as the same drop does
    // after an asynchronous lower whose event the guest has not taken
    // delivery of, so a `run` that completes is the proof that the
    // synchronous lower delivered the subtask's resolution before it
    // returned.
    let (mut store, instance) = lender().await;

    let result = func(&instance, "run")
        .call(&mut store, &[])
        .await
        .expect("the lend came back, so the owning handle drops");

    assert_eq!(
        result.first(),
        Some(&Val::U32(100)),
        "the guest ran past its drop of the owning handle"
    );
}
