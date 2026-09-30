//! Baseline tests for the failures that leave a store usable.
//!
//! A trap poisons the store, and a poisoned store refuses every guest
//! entry. The failures here are not traps: the polyfill raises each
//! one before any guest state changes, so each leaves the store as
//! it was, and a call into the guest afterwards goes through.
//!
//! One of them is the recursive-driver cause, and the only driver a
//! host can enter while a turn is in flight is one it builds from the
//! borrow an accessor lends, which is the crate's own surface. That is
//! why the test lives inside the crate.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use crate::store::StoreContextInternalExt;
use crate::{
    Component, Engine, Error, Instance, InstantiationError, Linker, SchedulerCause, Store, Val,
};
use wcmp_macros::component;

/// A component with one synchronous export, `ok`, which answers 7.
const OK: &[u8] = component!(
    r#"
    (component
      (core module $M
        (func (export "ok") (result i32) (i32.const 7)))
      (core instance $m (instantiate $M))
      (func (export "ok") (result u32)
        (canon lift (core func $m "ok"))))
    "#
);

/// A component that imports a function the linker never registers,
/// so its instantiation fails with a link error, and that would trap
/// in a core `start` function if it got that far.
const UNRESOLVED: &[u8] = component!(
    r#"
    (component
      (import "missing" (func $missing))
      (core module $M
        (func $start unreachable)
        (start $start))
      (core instance (instantiate $M)))
    "#
);

/// Poll `future` once with `waker`.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

/// Assert that a call of `ok` answers 7, so the store is usable.
async fn assert_usable(store: &mut Store<()>, instance: &Instance, after: &str) {
    let answered = instance
        .get_func("ok")
        .expect("`ok` is exported")
        .call(store, &[])
        .await
        .unwrap_or_else(|error| panic!("the store is usable after {after}, got {error}"));
    assert_eq!(answered.first(), Some(&Val::U32(7)), "after {after}");
}

#[wcmp_macros::test]
async fn it_leaves_the_store_usable_after_an_arity_mismatch_a_recursive_driver_or_a_link_error() {
    let engine = Engine::with_backend(crate::runtime_layer::test_backend()).expect("engine");
    let linker: Linker<()> = Linker::new(&engine);
    let component = Component::new(&engine, OK).await.expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    let ok = instance.get_func("ok").expect("`ok` is exported");

    // An arity mismatch is refused before the call creates a task.
    let error = ok
        .call(&mut store, &[Val::U32(1)])
        .await
        .expect_err("`ok` takes no argument");
    assert!(
        matches!(error, Error::Abi(_)),
        "the call is refused for its arity, got {error:?}"
    );
    assert_usable(&mut store, &instance, "an arity mismatch").await;

    // A driver entered while a turn is in flight is refused before it
    // queues anything.
    let refused = store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| {
                    let mut reborrowed = store.internal().reborrow();
                    let mut nested = Box::pin(reborrowed.internal().run_concurrent(async |_| ()));
                    match poll_once(&mut nested, Waker::noop()) {
                        Poll::Ready(Err(error)) => error.to_string(),
                        Poll::Ready(Ok(())) => "the driver succeeded".to_owned(),
                        Poll::Pending => "the driver returned pending".to_owned(),
                    }
                })
                .expect("reach the store")
        })
        .await
        .expect("run the closure");
    assert_eq!(
        refused,
        Error::Scheduler(SchedulerCause::RecursiveDriver).to_string(),
        "the nested driver is refused with the recursive-driver cause"
    );
    assert_usable(&mut store, &instance, "a recursive driver").await;

    // A link error is raised before the plan runs any guest code.
    let unresolved = Component::new(&engine, UNRESOLVED)
        .await
        .expect("component parses");
    let error = match linker.instantiate(&mut store, &unresolved).await {
        Ok(_) => panic!("`missing` resolves to no registration"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Link(_)),
        "the instantiation fails to link, got {error:?}"
    );
    assert_usable(&mut store, &instance, "a link error").await;

    // A call made through another store is refused before it reaches
    // either store.
    let mut other: Store<()> = Store::new(&engine, ()).expect("another store");
    let error = ok
        .call(&mut other, &[])
        .await
        .expect_err("the export belongs to the first store");
    assert!(
        matches!(&error, Error::Instantiation(inner) if matches!(**inner, InstantiationError::WrongStore)),
        "the call is refused for its store, got {error:?}"
    );
    assert_usable(&mut store, &instance, "a call through another store").await;
}
