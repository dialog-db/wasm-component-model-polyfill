//! Baseline tests for the store's `run_concurrent` entry and the
//! accessor it hands its closure.
//!
//! The entry is a driver: it polls the store's scheduler until the
//! closure's future completes. The closure does not borrow the
//! store, and neither does the accessor: the accessor is a token
//! carrying the store's identity, with no lifetime on it, so the
//! closure's future can hold one across its awaits and even carry
//! one out of the entry. It reaches the store's host data only
//! inside a closure the accessor runs, and only while a poll of the
//! entry's closure is running — a reach made anywhere else fails
//! with the store-not-in-poll cause. A driver entered from inside
//! that closure fails with the recursive-driver cause.
//!
//! A call into an export is not one of the drivers that can be
//! entered from there. The entry takes the store the host owns, and
//! the closure never holds it — the accessor lends a borrow of the
//! store as guest work reaches it, not the store itself — so the
//! refusal a call would meet is a shape the closure cannot even
//! write. What the closure can enter is another `run_concurrent`
//! against the borrow it was lent, and that is what the refusal is
//! read from here. Once the entry has returned the store is the
//! host's again, and a call into an export goes through.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use crate::internal::AccessorInternal;
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{Accessor, Component, Engine, Error, Linker, Result, SchedulerCause, Store, Val};
use wcmp_macros::component;

/// A component with one synchronous export, so that a call into it
/// is a driver.
const DOUBLES: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "double") (param i32) (result i32)
          local.get 0 i32.const 2 i32.mul))
      (core instance $i (instantiate $m))
      (func (export "double") (param "x" u32) (result u32)
        (canon lift (core func $i "double"))))
    "#
);

/// Poll `future` once, as an executor would.
fn poll_once<F: Future>(future: &mut Pin<Box<F>>, waker: &Waker) -> Poll<F::Output> {
    let mut context = Context::from_waker(waker);
    future.as_mut().poll(&mut context)
}

/// How a driver entered from inside the closure came out.
fn cause(outcome: Poll<Result<()>>) -> String {
    match outcome {
        Poll::Ready(Err(error)) => error.to_string(),
        Poll::Ready(Ok(())) => "the driver succeeded".to_owned(),
        Poll::Pending => "the driver returned pending".to_owned(),
    }
}

#[wcmp_macros::test]
async fn it_runs_its_closure_with_an_accessor_that_reaches_the_host_data() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<Vec<String>> =
        Store::new(&engine, vec!["first".to_owned()]).expect("store");

    let seen = store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| {
                    store.data_mut().push("second".to_owned());
                    // The borrow the closure is given outlives
                    // nothing, so what the host keeps is a clone.
                    store.data().clone()
                })
                .expect("reach the host data")
        })
        .await
        .expect("run the closure");

    assert_eq!(
        seen,
        vec!["first".to_owned(), "second".to_owned()],
        "the entry returns what the closure returned"
    );
    assert_eq!(
        store.data(),
        &vec!["first".to_owned(), "second".to_owned()],
        "what the closure wrote to the host data stayed in the store"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_driver_entered_from_inside_the_closure() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    let seen = store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| {
                    let mut reborrowed = store.internal().reborrow();
                    let mut nested = Box::pin(reborrowed.internal().run_concurrent(async |_| ()));
                    cause(poll_once(&mut nested, Waker::noop()))
                })
                .expect("reach the store")
        })
        .await
        .expect("run the closure");

    assert_eq!(
        seen,
        Error::Scheduler(SchedulerCause::RecursiveDriver).to_string(),
        "the store is inside a turn while the closure runs, so a driver \
         entered from there is refused"
    );
}

/// The message an accessor carries when it reached for its store
/// where no poll of that store was running. It is written out here
/// rather than built from the cause, so that a change to the words
/// is a change a test has to be told about.
const NOT_IN_POLL: &str = "scheduler error: an accessor reached its store outside a poll of that \
                           store";

#[wcmp_macros::test]
async fn it_refuses_a_reach_made_outside_any_poll() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<Vec<String>> = Store::new(&engine, Vec::new()).expect("store");

    // The accessor borrows nothing, so nothing stops it outliving
    // the entry that handed it over. What it reaches is the store
    // the running poll lent, and out here no poll is running.
    let escaped = store
        .run_concurrent(async |accessor| accessor.clone())
        .await
        .expect("run the closure");

    let refused = escaped
        .with(|store| store.data().len())
        .expect_err("the reach is refused");

    assert_eq!(
        refused.to_string(),
        NOT_IN_POLL,
        "the entry has returned, so no poll of the store is running"
    );
    assert!(
        matches!(refused, Error::Scheduler(SchedulerCause::StoreNotInPoll)),
        "and the cause is the store-not-in-poll one"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_reach_with_the_accessor_of_another_store() {
    let engine = Engine::new().expect("engine");
    let mut first: Store<Vec<String>> = Store::new(&engine, Vec::new()).expect("store");
    let mut second: Store<Vec<String>> = Store::new(&engine, Vec::new()).expect("store");

    let elsewhere = first
        .run_concurrent(async |accessor| accessor.clone())
        .await
        .expect("run the closure");

    // A poll is running here, but it lent the other store: one slot
    // holds one store, and this accessor does not name it.
    let seen = second
        .run_concurrent(async move |_accessor| {
            elsewhere
                .with(|store| store.data().len())
                .expect_err("the reach is refused")
                .to_string()
        })
        .await
        .expect("run the closure");

    assert_eq!(
        seen, NOT_IN_POLL,
        "the poll that is running lent another store, so the reach is refused"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_reach_with_an_accessor_typed_by_other_host_data() {
    let engine = Engine::new().expect("engine");
    let mut store: Store<Vec<String>> =
        Store::new(&engine, vec!["host data".to_owned()]).expect("store");

    // An accessor is built from a store's identity, which a host
    // holding the store can read, and the host picks the accessor's
    // host data type: nothing about this line is out of reach from
    // out here. The store's host data is a `Vec<String>` and this
    // accessor asks for a `u32`, so a reach that lent it the store
    // would read one as the other.
    let mistyped: Accessor<u32> = Accessor::new(store.internal().id());

    // A poll of this very store is running, so the identity in the
    // accessor matches what the slot names. The host data type does
    // not, and the reach is refused on that.
    let refused = store
        .run_concurrent(async move |_accessor| {
            let refused = mistyped
                .with(|store| *store.data())
                .expect_err("the reach is refused");
            (refused.to_string(), refused)
        })
        .await
        .expect("run the closure");

    assert_eq!(
        refused.0, NOT_IN_POLL,
        "no store with this identity and this host data is inside a poll"
    );
    assert!(
        matches!(refused.1, Error::Scheduler(SchedulerCause::StoreNotInPoll)),
        "and the cause is the store-not-in-poll one"
    );
}

#[wcmp_macros::test]
async fn it_calls_an_export_once_the_entry_has_returned() {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, DOUBLES)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    let double = instance.get_func("double").expect("double export");

    store
        .run_concurrent(async |accessor| {
            accessor.with(|_store| ()).expect("reach the store");
        })
        .await
        .expect("run the closure");

    // The entry gave the turn's mark back when it returned, so the
    // call is not the driver that is refused.
    let results = double
        .call(&mut store, &[Val::U32(21)])
        .await
        .expect("call the export");

    assert_eq!(
        results.as_ref(),
        [Val::U32(42)],
        "the store is the host's again once the entry has returned"
    );
}
