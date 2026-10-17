// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of the error options of `wcmp::bindgen!`:
//! `trappable_error_type`, whose `convert_*` gives the guest the WIT
//! error or traps it, on a plain method, a `store`-flag method, and an
//! async one, and `anyhow: true`, whose errors come back as the
//! polyfill error they hold or as `Error::Host`.

#![cfg(test)]

use wcmp::{
    Access, Accessor, Component, Engine, Error, HasSelf, Instance, Linker, ResourceTableError,
    Store, TaskCause,
};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A host's own error, which `read` returns and `convert_fs_error`
/// converts.
#[derive(Debug, PartialEq)]
pub enum ReadError {
    /// Converts to the WIT `not-found`.
    Missing,
    /// Has no WIT error, and traps.
    Broken,
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ReadError {}

mod mapped {
    wcmp::bindgen!({
        path: "wit-errors",
        world: "errors",
        imports: { default: trappable },
        trappable_error_type: { "test:errors/host.fs-error" => super::ReadError },
    });
}

mod with_anyhow {
    wcmp::bindgen!({
        path: "wit-errors",
        world: "errors",
        anyhow: true,
        imports: { default: trappable },
    });
}

mod reaching_the_store {
    wcmp::bindgen!({
        path: "wit-errors",
        world: "store-errors",
        imports: {
            "test:errors/store-host.read-store": store | trappable,
            default: trappable,
        },
        trappable_error_type: { "test:errors/store-host.fs-error" => super::ReadError },
    });
}

/// The core of each host's `read`: a value, a missing file, or a
/// broken one.
fn read(code: u32) -> Result<u32, ReadError> {
    match code {
        0 => Ok(7),
        1 => Err(ReadError::Missing),
        _ => Err(ReadError::Broken),
    }
}

/// The guest: each `run-*` export calls the import of its name and
/// returns what it returned.
const CALLS_THE_HOST: &[u8] = component!(
    r#"
    (component
      (import "test:errors/host" (instance $host
        (type $fs-error' (enum "not-found" "denied"))
        (export "fs-error" (type $fs-error (eq $fs-error')))
        (export "read" (func (param "code" u32) (result (result u32 (error $fs-error)))))
        (export "fail" (func (param "code" u32) (result u32)))
        (export "table-fail" (func (result u32)))))
      (alias export $host "read" (func $read))
      (alias export $host "fail" (func $fail))
      (alias export $host "table-fail" (func $table-fail))
      (alias export $host "fs-error" (type $fs-error))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $read-lower (canon lower (func $read) (memory (core memory $libc "memory"))))
      (core func $fail-lower (canon lower (func $fail)))
      (core func $table-fail-lower (canon lower (func $table-fail)))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "read" (func $read (param i32 i32)))
        (import "" "fail" (func $fail (param i32) (result i32)))
        (import "" "table-fail" (func $table-fail (result i32)))
        (func (export "run-read") (param i32) (result i32)
          (call $read (local.get 0) (i32.const 16))
          (i32.const 16))
        (func (export "run-fail") (param i32) (result i32) (call $fail (local.get 0)))
        (func (export "run-table-fail") (result i32) (call $table-fail)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "read" (func $read-lower))
          (export "fail" (func $fail-lower))
          (export "table-fail" (func $table-fail-lower))))))
      (func (export "run-read") (param "code" u32) (result (result u32 (error $fs-error)))
        (canon lift (core func $m "run-read") (memory (core memory $libc "memory"))))
      (func (export "run-fail") (param "code" u32) (result u32)
        (canon lift (core func $m "run-fail")))
      (func (export "run-table-fail") (result u32)
        (canon lift (core func $m "run-table-fail"))))
    "#
);

/// The host of [`mapped`].
struct Mapped;

impl mapped::test::errors::host::Host for Mapped {
    fn read(&mut self, code: u32) -> Result<u32, ReadError> {
        read(code)
    }

    fn fail(&mut self, code: u32) -> wcmp::Result<u32> {
        Ok(code)
    }

    fn table_fail(&mut self) -> wcmp::Result<u32> {
        Ok(0)
    }

    fn convert_fs_error(
        &mut self,
        err: ReadError,
    ) -> wcmp::Result<mapped::test::errors::host::FsError> {
        match err {
            ReadError::Missing => Ok(mapped::test::errors::host::FsError::NotFound),
            ReadError::Broken => Err(anyhow::Error::new(err))?,
        }
    }
}

/// The guest of [`reaching_the_store`]: `run-read-store` calls
/// `read-store` through a synchronous lower, and `run-read-async` calls
/// `read-async` through an asynchronous one and waits for a call that
/// starts. Each returns what the import returned.
const READS_THROUGH_THE_STORE: &[u8] = component!(
    r#"
    (component
      (import "test:errors/store-host" (instance $host
        (type $fs-error' (enum "not-found" "denied"))
        (export "fs-error" (type $fs-error (eq $fs-error')))
        (export "read-store" (func (param "code" u32) (result (result u32 (error $fs-error)))))
        (export "read-async"
          (func async (param "code" u32) (result (result u32 (error $fs-error)))))))
      (alias export $host "read-store" (func $read-store))
      (alias export $host "read-async" (func $read-async))
      (alias export $host "fs-error" (type $fs-error))
      (core module $libc (memory (export "memory") 1))
      (core instance $libc (instantiate $libc))
      (core func $read-store-lower
        (canon lower (func $read-store) (memory (core memory $libc "memory"))))
      (core func $read-async-lower
        (canon lower (func $read-async) async (memory (core memory $libc "memory"))))
      (core func $task-return (canon task.return (result (result u32 (error $fs-error)))))
      (core func $set-new (canon waitable-set.new))
      (core func $join (canon waitable.join))
      (core module $m
        (import "libc" "memory" (memory 1))
        (import "" "read-store" (func $read-store (param i32 i32)))
        (import "" "read-async" (func $read-async (param i32 i32) (result i32)))
        (import "" "task.return" (func $task-return (param i32 i32)))
        (import "" "waitable-set.new" (func $set-new (result i32)))
        (import "" "waitable.join" (func $join (param i32 i32)))
        (func $return-read
          (call $task-return (i32.load8_u (i32.const 0)) (i32.load (i32.const 4))))
        (func (export "run-read-store") (param i32) (result i32)
          (call $read-store (local.get 0) (i32.const 16))
          (i32.const 16))
        (func (export "run-read-async") (param i32) (result i32)
          (local $status i32)
          (local $set i32)
          (local.set $status (call $read-async (local.get 0) (i32.const 0)))
          (if (i32.eq (i32.and (local.get $status) (i32.const 0xf)) (i32.const 2))
            (then
              (call $return-read)
              (return (i32.const 0))))
          (local.set $set (call $set-new))
          (call $join (i32.shr_u (local.get $status) (i32.const 4)) (local.get $set))
          (i32.or (i32.shl (local.get $set) (i32.const 4)) (i32.const 2)))
        (func (export "read-async-callback") (param i32 i32 i32) (result i32)
          (call $return-read)
          (i32.const 0)))
      (core instance $m (instantiate $m
        (with "libc" (instance $libc))
        (with "" (instance
          (export "read-store" (func $read-store-lower))
          (export "read-async" (func $read-async-lower))
          (export "task.return" (func $task-return))
          (export "waitable-set.new" (func $set-new))
          (export "waitable.join" (func $join))))))
      (func (export "run-read-store") (param "code" u32) (result (result u32 (error $fs-error)))
        (canon lift (core func $m "run-read-store") (memory (core memory $libc "memory"))))
      (func (export "run-read-async") async
        (param "code" u32) (result (result u32 (error $fs-error)))
        (canon lift (core func $m "run-read-async") async
          (callback (core func $m "read-async-callback")))))
    "#
);

/// The host of [`reaching_the_store`]: the number of reads each method
/// served through the store.
#[derive(Default)]
struct Reaching {
    reads: u32,
}

impl reaching_the_store::test::errors::store_host::Host for Reaching {
    fn convert_fs_error(
        &mut self,
        err: ReadError,
    ) -> wcmp::Result<reaching_the_store::test::errors::store_host::FsError> {
        match err {
            ReadError::Missing => {
                Ok(reaching_the_store::test::errors::store_host::FsError::NotFound)
            }
            ReadError::Broken => Err(anyhow::Error::new(err))?,
        }
    }
}

impl reaching_the_store::test::errors::store_host::HostWithStore<Reaching> for HasSelf<Reaching> {
    fn read_store(mut host: Access<'_, Reaching, Self>, code: u32) -> Result<u32, ReadError> {
        host.get().reads += 1;
        read(code)
    }

    async fn read_async(accessor: &Accessor<Reaching, Self>, code: u32) -> Result<u32, ReadError> {
        accessor
            .with(|mut access| access.get().reads += 1)
            .expect("the future reaches the store");
        read(code)
    }
}

/// Instantiate [`READS_THROUGH_THE_STORE`] with a [`Reaching`] host.
async fn instantiate_reaching() -> (Store<Reaching>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, READS_THROUGH_THE_STORE)
        .await
        .expect("the component parses");
    let mut linker: Linker<Reaching> = Linker::new(&engine);
    reaching_the_store::test::errors::store_host::add_to_linker::<Reaching, HasSelf<Reaching>>(
        &mut linker,
        |host| host,
    )
    .expect("the registration");
    let mut store = Store::new(&engine, Reaching::default()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

/// Call the export `name` of [`READS_THROUGH_THE_STORE`] with `code`.
async fn run_reaching(
    store: &mut Store<Reaching>,
    instance: &Instance,
    name: &str,
    code: u32,
) -> wcmp::Result<Result<u32, reaching_the_store::test::errors::store_host::FsError>> {
    let (read,) = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the guest exports `{name}`"))
        .typed::<(u32,), (Result<u32, reaching_the_store::test::errors::store_host::FsError>,)>()?
        .call(store, (code,))
        .await?;
    Ok(read)
}

/// An error of the host's own type, which [`WithAnyhow`] fails with.
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

/// The host of [`with_anyhow`].
struct WithAnyhow;

impl with_anyhow::test::errors::host::Host for WithAnyhow {
    fn read(
        &mut self,
        _code: u32,
    ) -> anyhow::Result<Result<u32, with_anyhow::test::errors::host::FsError>> {
        Ok(Ok(0))
    }

    fn fail(&mut self, code: u32) -> anyhow::Result<u32> {
        Err(anyhow::Error::new(HostFailure { code }))
    }

    fn table_fail(&mut self) -> anyhow::Result<u32> {
        Err(anyhow::Error::new(Error::ResourceTable(
            ResourceTableError::NotPresent,
        )))
    }
}

/// Instantiate [`CALLS_THE_HOST`] with the host `T`, which `register`
/// registers.
async fn instantiate<T: wcmp::MaybeSend + 'static>(
    host: T,
    register: impl FnOnce(&mut Linker<T>),
) -> (Store<T>, Instance) {
    let engine = Engine::with_backend(crate::test_backend::backend()).expect("engine");
    let component = Component::new(&engine, CALLS_THE_HOST)
        .await
        .expect("the component parses");
    let mut linker: Linker<T> = Linker::new(&engine);
    register(&mut linker);
    let mut store = Store::new(&engine, host).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("the component instantiates");
    (store, instance)
}

/// Call `run-read` with `code`.
async fn run_read<T: 'static>(
    store: &mut Store<T>,
    instance: &Instance,
    code: u32,
) -> wcmp::Result<Result<u32, mapped::test::errors::host::FsError>> {
    let (read,) = instance
        .get_func("run-read")
        .expect("the guest exports `run-read`")
        .typed::<(u32,), (Result<u32, mapped::test::errors::host::FsError>,)>()?
        .call(store, (code,))
        .await?;
    Ok(read)
}

/// Call the export `name`, which may take a `u32` and returns one.
async fn call_u32<T: 'static>(
    store: &mut Store<T>,
    instance: &Instance,
    name: &str,
    args: &[u32],
) -> wcmp::Result<u32> {
    let values = args
        .iter()
        .map(|arg| wcmp::Val::U32(*arg))
        .collect::<Vec<_>>();
    let results = instance
        .get_func(name)
        .unwrap_or_else(|| panic!("the guest exports `{name}`"))
        .call(store, &values)
        .await?;
    match results.first() {
        Some(wcmp::Val::U32(value)) => Ok(*value),
        other => panic!("`{name}` returned {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_gives_the_guest_the_wit_error_convert_returns() {
    let (mut store, instance) = instantiate(Mapped, |linker| {
        mapped::test::errors::host::add_to_linker::<Mapped, HasSelf<Mapped>>(linker, |host| host)
            .expect("the registration");
    })
    .await;

    assert_eq!(
        run_read(&mut store, &instance, 0)
            .await
            .expect("call run-read"),
        Ok(7),
        "the method's `Ok` reaches the guest"
    );
    assert_eq!(
        run_read(&mut store, &instance, 1)
            .await
            .expect("call run-read"),
        Err(mapped::test::errors::host::FsError::NotFound),
        "`convert_fs_error`'s `Ok` reaches the guest as the WIT error"
    );
}

#[wcmp_macros::test]
async fn it_traps_the_guest_when_convert_fails() {
    let (mut store, instance) = instantiate(Mapped, |linker| {
        mapped::test::errors::host::add_to_linker::<Mapped, HasSelf<Mapped>>(linker, |host| host)
            .expect("the registration");
    })
    .await;

    let error = run_read(&mut store, &instance, 2)
        .await
        .expect_err("`convert_fs_error` fails, so the guest traps");
    assert_eq!(
        std::error::Error::source(&error).and_then(|source| source.downcast_ref::<ReadError>()),
        Some(&ReadError::Broken),
        "the call returns the error `convert_fs_error` failed with, got {error:?}"
    );
    let after = run_read(&mut store, &instance, 0)
        .await
        .expect_err("the trap poisoned the store");
    assert!(
        matches!(after, Error::Task(TaskCause::CannotEnter)),
        "got {after:?}"
    );
}

#[wcmp_macros::test]
async fn it_keeps_the_error_of_an_anyhow_method() {
    let register = |linker: &mut Linker<WithAnyhow>| {
        with_anyhow::test::errors::host::add_to_linker::<WithAnyhow, HasSelf<WithAnyhow>>(
            linker,
            |host| host,
        )
        .expect("the registration");
    };

    let (mut store, instance) = instantiate(WithAnyhow, register).await;
    let error = call_u32(&mut store, &instance, "run-fail", &[4])
        .await
        .expect_err("the host's error traps the guest");
    assert!(matches!(error, Error::Host(_)), "got {error:?}");
    assert_eq!(
        std::error::Error::source(&error).and_then(|source| source.downcast_ref::<HostFailure>()),
        Some(&HostFailure { code: 4 }),
        "the source downcasts to the host's own type"
    );

    let (mut store, instance) = instantiate(WithAnyhow, register).await;
    let error = call_u32(&mut store, &instance, "run-table-fail", &[])
        .await
        .expect_err("the polyfill error traps the guest");
    assert!(
        matches!(error, Error::ResourceTable(ResourceTableError::NotPresent)),
        "the polyfill error the `anyhow::Error` held comes back as itself, got {error:?}"
    );
}

#[wcmp_macros::test]
async fn it_converts_the_error_of_a_store_flag_method_and_of_an_async_method() {
    for name in ["run-read-store", "run-read-async"] {
        let (mut store, instance) = instantiate_reaching().await;
        assert_eq!(
            run_reaching(&mut store, &instance, name, 0)
                .await
                .unwrap_or_else(|error| panic!("call {name}: {error:?}")),
            Ok(7),
            "the method's `Ok` reaches the guest through {name}"
        );
        assert_eq!(
            run_reaching(&mut store, &instance, name, 1)
                .await
                .unwrap_or_else(|error| panic!("call {name}: {error:?}")),
            Err(reaching_the_store::test::errors::store_host::FsError::NotFound),
            "`convert_fs_error`'s `Ok` reaches the guest through {name}"
        );
        assert_eq!(
            store.data().reads,
            2,
            "each call of {name} reached the store"
        );
    }
}

#[wcmp_macros::test]
async fn it_traps_a_store_flag_method_and_an_async_method_when_convert_fails() {
    for name in ["run-read-store", "run-read-async"] {
        let (mut store, instance) = instantiate_reaching().await;
        let error = run_reaching(&mut store, &instance, name, 2)
            .await
            .expect_err("`convert_fs_error` fails, so the guest traps");
        assert_eq!(
            std::error::Error::source(&error).and_then(|source| source.downcast_ref::<ReadError>()),
            Some(&ReadError::Broken),
            "{name} returns the error `convert_fs_error` failed with, got {error:?}"
        );
    }
}

#[path = "../../wcmp/tests/support/backend.rs"]
mod test_backend;
