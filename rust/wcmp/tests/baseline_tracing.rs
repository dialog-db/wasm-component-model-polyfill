// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Baseline tests for the polyfill's spans.
//!
//! The polyfill and the runtime layer open a `tracing` span on each
//! critical path: the compile of a component, its instantiation, and a
//! call, down to the canonical ABI's lowering and lifting. Nothing
//! records them until an application installs a subscriber. These tests
//! install one for the length of a test, on the test's own thread, and
//! read back the names of the spans it saw.

#![cfg(test)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use wcmp::{Component, Engine, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A subscriber that keeps the name of every span opened under it, at
/// every level, in the order they opened.
#[derive(Clone, Default)]
struct Recorder {
    names: Arc<Mutex<Vec<String>>>,
    next: Arc<AtomicU64>,
}

impl Recorder {
    /// The names of the spans opened so far.
    fn names(&self) -> Vec<String> {
        self.names.lock().expect("the names").clone()
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        self.names
            .lock()
            .expect("the names")
            .push(span.metadata().name().to_string());
        Id::from_u64(self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// A component with one export, `answer`, which returns 42.
const ANSWER: &[u8] = component!(
    r#"
    (component
      (core module $m
        (func (export "answer") (result i32) i32.const 42))
      (core instance $i (instantiate $m))
      (func (export "answer") (result u32)
        (canon lift (core func $i "answer"))))
    "#
);

/// Whether `names` holds each of `expected`.
fn missing<'a>(names: &[String], expected: &[&'a str]) -> Vec<&'a str> {
    expected
        .iter()
        .copied()
        .filter(|name| !names.iter().any(|seen| seen == name))
        .collect()
}

#[wcmp_macros::test]
async fn it_opens_a_span_for_each_step_of_a_compile_an_instantiation_and_a_call() {
    let recorder = Recorder::default();
    let _default = tracing::subscriber::set_default(recorder.clone());

    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ANSWER)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let answer = instance
        .get_func("answer")
        .expect("the export")
        .call(&mut store, &[])
        .await
        .expect("the call returns");
    assert_eq!(&*answer, &[Val::U32(42)]);

    let names = recorder.names();
    let missing = missing(
        &names,
        &[
            "Component::new",
            "component parse, validate, and adapt",
            "core module compiles",
            "core module compile",
            "Linker::instantiate",
            "resolve imports",
            "run instantiation plan",
            "instantiate core module",
            "core instantiate",
            "Func::call",
            "lower arguments",
            "lift result",
        ],
    );
    assert!(missing.is_empty(), "no span {missing:?} among {names:?}");
}

#[wcmp_macros::test]
async fn it_opens_a_span_for_a_concurrent_call_and_the_driver_that_runs_it() {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, ANSWER)
        .await
        .expect("the component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiation succeeds");
    let func = instance.get_func("answer").expect("the export");

    let recorder = Recorder::default();
    let _default = tracing::subscriber::set_default(recorder.clone());
    let answer = store
        .run_concurrent(async |accessor| func.call_concurrent(accessor, &[]).await)
        .await
        .expect("the driver runs")
        .expect("the call returns");
    assert_eq!(&*answer, &[Val::U32(42)]);

    let names = recorder.names();
    let missing = missing(
        &names,
        &[
            "Store::run_concurrent",
            "Func::call_concurrent",
            "driver turn",
            "turn",
            "turn item",
        ],
    );
    assert!(missing.is_empty(), "no span {missing:?} among {names:?}");
    assert!(
        !names.iter().any(|name| name == "Component::new"),
        "a span from before the subscriber was installed: {names:?}"
    );
}

#[path = "support/backend.rs"]
mod test_backend;
