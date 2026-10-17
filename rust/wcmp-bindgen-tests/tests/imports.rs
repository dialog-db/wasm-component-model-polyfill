// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the imports `wcmp::bindgen!` writes: the `Host` and
//! `HostWithStore<T>` traits, `add_to_linker`, and the flag rules of the
//! `imports` option.
//!
//! The host implements the generated traits, registers them with
//! `add_to_linker`, and a guest calls each import: a plain function
//! that takes a variant and a flags value and returns a record, a
//! `trappable` one that fails with the host's own error, and one with
//! the `store` flag that makes a future through the store context and
//! returns it.

#![cfg(test)]

use wcmp::{
    Access, Component, Engine, Error, FutureReader, HasSelf, Instance, Linker, Store, TaskCause,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

mod bindings {
    wcmp::bindgen!({
        path: "wit-imports",
        additional_derives: [PartialEq],
        imports: {
            "test:imports/host.fail": trappable,
            "test:imports/host.lost": store | trappable,
        },
    });
}

/// The first rule that matches a function applies: `describe` is
/// `trappable` here, and the later `store` rule for it is not used.
mod first_rule_wins {
    wcmp::bindgen!({
        path: "wit-imports",
        imports: {
            "test:imports/host.describe": trappable,
            "test:imports/host": store,
        },
    });
}

use bindings::test::imports::host::{self, Permissions, Report, Shape};

/// An error of the host's own type.
#[derive(Debug, PartialEq)]
struct HostFailure {
    code: u32,
}

impl core::fmt::Display for HostFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "the host failed with code {}", self.code)
    }
}

impl std::error::Error for HostFailure {}

/// The host data, which records what the host functions were given.
#[derive(Default)]
struct HostState {
    described: Vec<(Shape, Permissions)>,
}

impl host::Host for HostState {
    fn describe(&mut self, shape: Shape, permissions: Permissions) -> Report {
        self.described.push((shape, permissions));
        let area = match shape {
            Shape::Circle(radius) => radius * radius,
            Shape::None => 0,
        };
        Report {
            area,
            writable: permissions.contains(Permissions::WRITE),
        }
    }

    fn fail(&mut self, code: u32) -> wcmp::Result<u32> {
        Err(anyhow::Error::new(HostFailure { code }))?
    }

    fn gated(&mut self) -> u32 {
        7
    }
}

impl host::HostWithStore<HostState> for HasSelf<HostState> {
    fn lost(mut host: Access<'_, HostState, Self>) -> wcmp::Result<FutureReader<u32>> {
        FutureReader::new(host.store(), async { Ok::<u32, Error>(42) })
    }
}

/// A guest that calls each import of `test:imports/host` but `gated`.
///
/// `run-describe` calls `describe` with `circle(r)` and both
/// permissions and returns the record it got; `run-fail` and `run-lost`
/// call `fail` and `lost`. `future-read` reads a future end into
/// memory, and `peek` reads a word of memory.
const CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "test:imports/host" (instance $host
        (type $shape' (variant (case "circle" u32) (case "none")))
        (export "shape" (type $shape (eq $shape')))
        (type $permissions' (flags "read" "write"))
        (export "permissions" (type $permissions (eq $permissions')))
        (type $report' (record (field "area" u32) (field "writable" bool)))
        (export "report" (type $report (eq $report')))
        (export "describe" (func
          (param "shape" $shape) (param "permissions" $permissions) (result $report)))
        (export "fail" (func (param "code" u32) (result u32)))
        (type $lost (future u32))
        (export "lost" (func (result $lost)))))
      (alias export $host "describe" (func $describe))
      (alias export $host "fail" (func $fail))
      (alias export $host "lost" (func $lost))
      (alias export $host "report" (type $report))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $describe-lower
        (canon lower (func $describe) (memory (core memory $libc "memory"))))
      (core func $fail-lower (canon lower (func $fail)))
      (core func $lost-lower (canon lower (func $lost)))
      (type $f (future u32))
      (core func $future-read
        (canon future.read $f async (memory (core memory $libc "memory"))))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "describe" (func $describe (param i32 i32 i32 i32)))
        (import "" "fail" (func $fail (param i32) (result i32)))
        (import "" "lost" (func $lost (result i32)))
        (import "" "future.read" (func $future-read (param i32 i32) (result i32)))
        (func (export "run-describe") (param $r i32) (result i32)
          (call $describe (i32.const 0) (local.get $r) (i32.const 3) (i32.const 16))
          (i32.const 16))
        (func (export "run-fail") (param i32) (result i32) (call $fail (local.get 0)))
        (func (export "run-lost") (result i32) (call $lost))
        (func (export "future-read") (param i32 i32) (result i32)
          (call $future-read (local.get 0) (local.get 1)))
        (func (export "peek") (param i32) (result i32) (i32.load (local.get 0))))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "describe" (func $describe-lower))
          (export "fail" (func $fail-lower))
          (export "lost" (func $lost-lower))
          (export "future.read" (func $future-read))))))
      (func (export "run-describe") (param "r" u32) (result $report)
        (canon lift (core func $m "run-describe") (memory (core memory $libc "memory"))))
      (func (export "run-fail") (param "code" u32) (result u32)
        (canon lift (core func $m "run-fail")))
      (func (export "run-lost") (result u32) (canon lift (core func $m "run-lost")))
      (func (export "future-read") (param "e" u32) (param "p" u32) (result u32)
        (canon lift (core func $m "future-read")))
      (func (export "peek") (param "p" u32) (result u32) (canon lift (core func $m "peek"))))
    "#
);

/// Instantiate [`CALLS_THE_HOST`] against the generated registrations.
async fn instantiate() -> (Store<HostState>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, CALLS_THE_HOST)
        .await
        .expect("the component parses");
    let mut linker: Linker<HostState> = Linker::new(&engine);
    host::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)
        .expect("the generated registration");
    let mut store = Store::new(&engine, HostState::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component links without importing `gated`, and instantiates");
    (store, instance)
}

/// Call the export `name`, which takes and returns `u32`s.
async fn call_u32(
    store: &mut Store<HostState>,
    instance: &Instance,
    name: &str,
    args: &[u32],
) -> wcmp::Result<u32> {
    let func = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the guest exports `{name}`"));
    let values = args
        .iter()
        .map(|arg| wcmp::Val::U32(*arg))
        .collect::<Vec<_>>();
    let results = func.call(store, &values).await?;
    match results.first() {
        Some(wcmp::Val::U32(value)) => Ok(*value),
        other => panic!("`{name}` returned {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_hands_a_generated_import_a_variant_and_flags_and_returns_a_record() {
    let (mut store, instance) = instantiate().await;

    let (report,) = instance
        .get_func("run-describe")
        .expect("the guest exports `run-describe`")
        .typed::<(u32,), (Report,)>()
        .expect("the export's type")
        .call(&mut store, (3,))
        .await
        .expect("call run-describe");

    assert_eq!(
        report,
        Report {
            area: 9,
            writable: true
        },
        "the guest received the record the host returned"
    );
    assert_eq!(
        store.data().described,
        [(Shape::Circle(3), Permissions::READ | Permissions::WRITE)],
        "the host received the variant and the flags the guest passed"
    );
}

#[wcmp_macros::test]
async fn it_traps_the_guest_on_a_host_error_from_a_generated_method() {
    let (mut store, instance) = instantiate().await;

    let error = call_u32(&mut store, &instance, "run-fail", &[5])
        .await
        .expect_err("the host's error traps the guest");
    assert!(
        matches!(error, Error::Host(_)),
        "the call returns the host's error, got {error:?}"
    );
    assert_eq!(
        std::error::Error::source(&error).and_then(|source| source.downcast_ref::<HostFailure>()),
        Some(&HostFailure { code: 5 }),
        "the source downcasts to the host's own type"
    );

    let after = call_u32(&mut store, &instance, "run-fail", &[5])
        .await
        .expect_err("the trap poisoned the store");
    assert!(
        matches!(after, Error::Task(TaskCause::CannotEnter)),
        "a later call is refused, got {after:?}"
    );
}

#[wcmp_macros::test]
async fn it_returns_a_future_a_store_flag_method_made_through_its_access() {
    let (mut store, instance) = instantiate().await;

    let end = call_u32(&mut store, &instance, "run-lost", &[])
        .await
        .expect("call run-lost");
    let read = call_u32(&mut store, &instance, "future-read", &[end, 100])
        .await
        .expect("read the future");
    assert_eq!(read, 0, "the read completed at once: the future was ready");
    assert_eq!(
        call_u32(&mut store, &instance, "peek", &[100])
            .await
            .expect("peek"),
        42,
        "the guest read the value the host's future produced"
    );
}

#[wcmp_macros::test]
fn it_generates_a_gated_import_and_registers_it() {
    // The method is part of the trait, so the host above implements
    // it, and the registration links a component that does not import
    // it, which `instantiate` shows.
    let mut state = HostState::default();
    assert_eq!(host::Host::gated(&mut state), 7);
}

/// The host of [`first_rule_wins`]: `describe` returns a `Result`,
/// because the first rule that matched it was `trappable`, and it takes
/// `&mut self`, because the later `store` rule did not apply to it.
struct FirstRule;

impl first_rule_wins::test::imports::host::Host for FirstRule {
    fn describe(
        &mut self,
        _shape: first_rule_wins::test::imports::host::Shape,
        _permissions: first_rule_wins::test::imports::host::Permissions,
    ) -> wcmp::Result<first_rule_wins::test::imports::host::Report> {
        Ok(first_rule_wins::test::imports::host::Report {
            area: 0,
            writable: false,
        })
    }
}

impl first_rule_wins::test::imports::host::HostWithStore<FirstRule> for HasSelf<FirstRule> {
    fn fail(_host: Access<'_, FirstRule, Self>, code: u32) -> u32 {
        code
    }

    fn lost(mut host: Access<'_, FirstRule, Self>) -> FutureReader<u32> {
        FutureReader::new(host.store(), async { Ok::<u32, Error>(0) }).expect("a future")
    }

    fn gated(_host: Access<'_, FirstRule, Self>) -> u32 {
        0
    }
}

#[wcmp_macros::test]
fn it_applies_the_first_rule_that_matches_a_function() {
    let mut host = FirstRule;
    let report = first_rule_wins::test::imports::host::Host::describe(
        &mut host,
        first_rule_wins::test::imports::host::Shape::None,
        first_rule_wins::test::imports::host::Permissions::empty(),
    );
    assert!(report.is_ok(), "`describe` is trappable");
}

#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
