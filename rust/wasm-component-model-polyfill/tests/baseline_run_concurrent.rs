//! Baseline tests for the store's `run_concurrent` entry and the
//! accessor it hands its closure.
//!
//! The entry is a driver: it polls the store's scheduler until the
//! closure's future completes. The closure does not borrow the
//! store. It reaches the store's host data only inside a closure the
//! accessor runs, and a driver entered from inside that closure —
//! here a call into an export — fails with the recursive-driver
//! cause.

#![cfg(test)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use wasm_component_model_polyfill::{
    Component, Engine, Error, Linker, Result, SchedulerCause, Store, Val,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

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
fn cause(outcome: Poll<Result<Box<[Val]>>>) -> String {
    match outcome {
        Poll::Ready(Err(error)) => error.to_string(),
        Poll::Ready(Ok(_)) => "the call succeeded".to_owned(),
        Poll::Pending => "the call returned pending".to_owned(),
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
async fn it_refuses_a_call_into_an_export_entered_from_inside_the_closure() {
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

    let seen = store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| {
                    let mut call = Box::pin(double.call(store, &[Val::U32(21)]));
                    cause(poll_once(&mut call, Waker::noop()))
                })
                .expect("reach the store")
        })
        .await
        .expect("run the closure");

    assert_eq!(
        seen,
        Error::Scheduler(SchedulerCause::RecursiveDriver).to_string(),
        "the store is inside a turn while the closure runs, so a call into \
         an export entered from there is refused"
    );
}
