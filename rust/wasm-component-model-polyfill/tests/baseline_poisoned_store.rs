//! Baseline tests for the poisoned store.
//!
//! A trap poisons the store, as Wasmtime keeps one trapped flag per
//! store, and a poisoned store runs no more guest code. Every host
//! entry into a guest fails with Wasmtime's cannot-enter trap, "cannot
//! enter component instance": a call, a concurrent call, an
//! instantiation, and the release of a resource a guest defines. What
//! runs no guest code still works: the release of a resource the host
//! defines, and a `run_concurrent` whose closure does only host work.
//!
//! The tests drive one component with an export for each trigger and
//! read the poison back through the one entry every test shares, a
//! call of `ok`, which returns 7 in a store no trap has poisoned.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasm_component_model_polyfill::{
    Component, Engine, Error, FunctionType, HostCall, Instance, Linker, Module, ResourceHandle,
    ResourceTypeId, Store, TaskCause, Val,
};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// A component with one export per trigger of the poisoned store, and
/// `ok`, which runs no trap at all.
///
/// - `unreachable` traps in its own core code.
/// - `builtin-trap` drops a handle index its table never gave out,
///   which traps inside the `resource.drop` built-in.
/// - `host-fail` calls the host's `fail` through a synchronous lower,
///   and the host function fails.
/// - `take` takes a `u32`, so a host [`Val`] of another type fails
///   as the call lowers it.
/// - `make` mints a resource of the component's own type `r`, whose
///   destructor traps for the rep 13 and returns for any other.
///
/// The component also imports the host's resource `thing`, so that
/// the host's destructor for it is the store's to run.
const TRIGGERS: &[u8] = component!(
    r#"
    (component
      (import "fail" (func $fail))
      (import "thing" (type $thing (sub resource)))
      (core func $fail' (canon lower (func $fail)))
      (core module $Dtor
        (func (export "dtor") (param i32)
          (if (i32.eq (local.get 0) (i32.const 13))
            (then unreachable))))
      (core instance $dtor (instantiate $Dtor))
      (type $r (resource (rep i32) (dtor (core func $dtor "dtor"))))
      (core func $new (canon resource.new $r))
      (core func $drop (canon resource.drop $r))
      (core module $M
        (import "" "fail" (func $fail))
        (import "" "new" (func $new (param i32) (result i32)))
        (import "" "drop" (func $drop (param i32)))
        (func (export "ok") (result i32) (i32.const 7))
        (func (export "unreachable") unreachable)
        (func (export "builtin-trap") (call $drop (i32.const 99)))
        (func (export "host-fail") (call $fail))
        (func (export "take") (param i32))
        (func (export "make") (param i32) (result i32)
          (call $new (local.get 0))))
      (core instance $m (instantiate $M (with "" (instance
        (export "fail" (func $fail'))
        (export "new" (func $new))
        (export "drop" (func $drop))))))
      (export $t "r" (type $r))
      (func (export "ok") (result u32)
        (canon lift (core func $m "ok")))
      (func (export "unreachable")
        (canon lift (core func $m "unreachable")))
      (func (export "builtin-trap")
        (canon lift (core func $m "builtin-trap")))
      (func (export "host-fail")
        (canon lift (core func $m "host-fail")))
      (func (export "take") (param "x" u32)
        (canon lift (core func $m "take")))
      (func (export "make") (param "rep" u32) (result (own $t))
        (canon lift (core func $m "make"))))
    "#
);

/// A component whose one core module traps in its `start` function.
const TRAPS_IN_START: &[u8] = component!(
    r#"
    (component
      (core module $M
        (func $start unreachable)
        (start $start))
      (core instance (instantiate $M)))
    "#
);

/// A component that runs no guest code as it instantiates, for the
/// instantiation a poisoned store refuses.
const EMPTY: &[u8] = component!(
    r#"
    (component
      (core module $M (func (export "f")))
      (core instance (instantiate $M)))
    "#
);

/// A core module whose `start` function traps.
const MODULE_TRAPS_IN_START: &[u8] = wasm!(
    r#"
    (module
      (func $start unreachable)
      (start $start))
    "#
);

/// A core module with no `start` function, which runs no guest code
/// as it instantiates.
const MODULE_WITHOUT_START: &[u8] = wasm!(
    r#"
    (module (func (export "f")))
    "#
);

/// The message of Wasmtime's cannot-enter trap, written out so that a
/// change to the words is a change a test is told about.
const CANNOT_ENTER: &str = "cannot enter component instance";

/// The host data of every store: how many times the host's
/// destructor for `thing` ran.
#[derive(Default)]
struct Drops(Arc<AtomicU32>);

/// A store with [`TRIGGERS`] instantiated in it, the linker that
/// instantiated it, and the type of the host's resource `thing`.
struct Fixture {
    engine: Engine,
    linker: Linker<Drops>,
    store: Store<Drops>,
    instance: Instance,
    thing: ResourceTypeId,
    drops: Arc<AtomicU32>,
}

impl Fixture {
    async fn new() -> Self {
        let engine = Engine::new().expect("engine");
        let component = Component::new(&engine, TRIGGERS)
            .await
            .expect("component parses");
        let mut linker: Linker<Drops> = Linker::new(&engine);
        let mut root = linker.root();
        root.func_new(
            "fail",
            FunctionType {
                parameters: Vec::new(),
                result: None,
                async_: false,
            },
            |_: HostCall<'_, Drops>, _args, _results| {
                Err(Error::Internal {
                    message: "the host function failed".to_owned(),
                })
            },
        )
        .expect("the registration of `fail`");
        let thing = root
            .resource("thing", |data: &mut Drops, _rep: u32| {
                data.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .expect("the registration of `thing`");
        let drops = Arc::new(AtomicU32::new(0));
        let mut store = Store::new(&engine, Drops(drops.clone())).expect("store");
        let instance = linker
            .instantiate(&mut store, &component)
            .await
            .expect("the component instantiates");
        Self {
            engine,
            linker,
            store,
            instance,
            thing,
            drops,
        }
    }

    /// Call the export `name` with `args`.
    async fn call(&mut self, name: &str, args: &[Val]) -> Result<Box<[Val]>, Error> {
        let func = self
            .instance
            .get_func(name)
            .unwrap_or_else(|| panic!("`{name}` is exported"));
        func.call(&mut self.store, args).await
    }

    /// A resource of the component's own type, minted by `make`.
    async fn make(&mut self, rep: u32) -> ResourceHandle {
        let made = self
            .call("make", &[Val::U32(rep)])
            .await
            .expect("`make` mints a resource");
        let Some(Val::Own(handle)) = made.first().cloned() else {
            panic!("`make` answers with an owned handle, got {made:?}");
        };
        handle
    }

    /// Assert that a call of `ok` answers 7, so the store is usable.
    async fn assert_usable(&mut self, after: &str) {
        let answered = self
            .call("ok", &[])
            .await
            .unwrap_or_else(|error| panic!("the store is usable after {after}, got {error}"));
        assert_eq!(answered.first(), Some(&Val::U32(7)), "after {after}");
    }

    /// Assert that a call of `ok` fails with the cannot-enter cause,
    /// so a trap poisoned the store.
    async fn assert_poisoned(&mut self, after: &str) {
        let error = self
            .call("ok", &[])
            .await
            .expect_err("a poisoned store refuses the call");
        assert_cannot_enter(&error, &format!("a call after {after}"));
    }
}

/// Assert that `error` is the cannot-enter cause, with its message.
fn assert_cannot_enter(error: &Error, entry: &str) {
    assert!(
        matches!(error, Error::Task(TaskCause::CannotEnter)),
        "{entry} fails with the cannot-enter cause, got {error:?}"
    );
    assert!(
        error.to_string().contains(CANNOT_ENTER),
        "{entry} fails with Wasmtime's message, got {error}"
    );
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_guest_trap() {
    let mut fixture = Fixture::new().await;
    fixture.assert_usable("instantiation").await;

    fixture
        .call("unreachable", &[])
        .await
        .expect_err("the guest traps");

    fixture.assert_poisoned("a guest trap").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_built_in_trap() {
    let mut fixture = Fixture::new().await;

    fixture
        .call("builtin-trap", &[])
        .await
        .expect_err("`resource.drop` of a handle the table never gave out traps");

    fixture.assert_poisoned("a built-in trap").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_failed_host_function_under_a_synchronous_lower() {
    let mut fixture = Fixture::new().await;

    let error = fixture
        .call("host-fail", &[])
        .await
        .expect_err("the host function fails the guest's call");
    assert!(
        format!("{error:?}").contains("the host function failed"),
        "the call reports the host function's failure, got {error:?}"
    );

    fixture.assert_poisoned("a failed host function").await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_host_value_of_the_wrong_type() {
    // Wasmtime lowers the arguments after its entry check, so a value
    // of the wrong type fails a call that has already entered the
    // guest's store, and poisons it. The polyfill matches that.
    let mut fixture = Fixture::new().await;

    let error = fixture
        .call("take", &[Val::String("seven".into())])
        .await
        .expect_err("a string is not a `u32`");
    assert!(
        matches!(error, Error::Abi(_)),
        "the lowering refuses the value, got {error:?}"
    );

    fixture
        .assert_poisoned("a host value of the wrong type")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_components_core_start_function() {
    let mut fixture = Fixture::new().await;
    let traps = Component::new(&fixture.engine, TRAPS_IN_START)
        .await
        .expect("component parses");

    let error = match fixture.linker.instantiate(&mut fixture.store, &traps).await {
        Ok(_) => panic!("a start function that traps fails the instantiation"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Instantiation(_)),
        "the start function's trap fails the instantiation, got {error:?}"
    );

    fixture
        .assert_poisoned("a trap in a component's core start function")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_host_modules_start_function() {
    let mut fixture = Fixture::new().await;
    let module = Module::new(&fixture.engine, MODULE_TRAPS_IN_START)
        .await
        .expect("the module compiles");

    module
        .instantiate(&mut fixture.store, &[])
        .await
        .expect_err("a start function that traps fails the instantiation");

    fixture
        .assert_poisoned("a trap in a host module's start function")
        .await;
}

#[wcmp_macros::test]
async fn it_poisons_the_store_on_a_trap_in_a_destructor() {
    let mut fixture = Fixture::new().await;
    let handle = fixture.make(13).await;

    fixture
        .store
        .resource_drop(handle)
        .expect_err("the destructor traps for the rep 13");

    fixture.assert_poisoned("a trap in a destructor").await;
}

#[wcmp_macros::test]
async fn it_leaves_the_store_usable_after_a_destructor_that_returns() {
    // The release that poisons above differs from this one only in
    // the rep, so the trap is what poisons, not the release.
    let mut fixture = Fixture::new().await;
    let handle = fixture.make(5).await;

    fixture
        .store
        .resource_drop(handle)
        .expect("the destructor returns for the rep 5");

    fixture.assert_usable("a destructor that returned").await;
}

#[wcmp_macros::test]
async fn it_refuses_every_guest_entry_of_a_poisoned_store() {
    let mut fixture = Fixture::new().await;
    // A resource of each kind the host holds, minted while the store
    // is still usable.
    let guest_resource = fixture.make(5).await;
    let host_resource = fixture
        .store
        .resource_new(fixture.thing, 42)
        .expect("the host mints its own resource");
    let ok = fixture.instance.get_func("ok").expect("`ok` is exported");
    let typed = fixture
        .instance
        .get_func("ok")
        .expect("`ok` is exported")
        .typed::<(), u32>()
        .expect("`ok` is `() -> u32`");

    fixture
        .call("unreachable", &[])
        .await
        .expect_err("the guest traps");

    // A call, untyped and typed.
    let error = ok
        .call(&mut fixture.store, &[])
        .await
        .expect_err("a poisoned store refuses `Func::call`");
    assert_cannot_enter(&error, "`Func::call`");
    let error = typed
        .call(&mut fixture.store, ())
        .await
        .expect_err("a poisoned store refuses `TypedFunc::call`");
    assert_cannot_enter(&error, "`TypedFunc::call`");

    // A concurrent call, untyped and typed. The closure runs, and the
    // refusal is the call's own: the entry around it returns.
    let (untyped, typed_concurrent) = fixture
        .store
        .run_concurrent(async |accessor| {
            let untyped = ok.call_concurrent(accessor, &[]).await;
            let typed_concurrent = typed.call_concurrent(accessor, ()).await;
            (untyped, typed_concurrent)
        })
        .await
        .expect("the entry around the concurrent calls returns");
    assert_cannot_enter(
        &untyped.expect_err("a poisoned store refuses `Func::call_concurrent`"),
        "`Func::call_concurrent`",
    );
    assert_cannot_enter(
        &typed_concurrent.expect_err("a poisoned store refuses `TypedFunc::call_concurrent`"),
        "`TypedFunc::call_concurrent`",
    );

    // An instantiation, of a component and of a core module, each of
    // which would run no guest code of its own.
    let empty = Component::new(&fixture.engine, EMPTY)
        .await
        .expect("component parses");
    let error = match fixture.linker.instantiate(&mut fixture.store, &empty).await {
        Ok(_) => panic!("a poisoned store refuses `Linker::instantiate`"),
        Err(error) => error,
    };
    assert_cannot_enter(&error, "`Linker::instantiate`");
    let module = Module::new(&fixture.engine, MODULE_WITHOUT_START)
        .await
        .expect("the module compiles");
    let error = module
        .instantiate(&mut fixture.store, &[])
        .await
        .expect_err("a poisoned store refuses a core module's instantiation");
    assert_cannot_enter(&error, "`Module::instantiate`");

    // The release of a resource the guest defines runs guest code;
    // the release of the host's own runs the host's destructor.
    let error = fixture
        .store
        .resource_drop(guest_resource)
        .expect_err("a poisoned store refuses the release of a guest's resource");
    assert_cannot_enter(&error, "`Store::resource_drop` of a guest's resource");
    fixture
        .store
        .resource_drop(host_resource)
        .expect("the release of the host's resource runs no guest code");
    assert_eq!(
        fixture.drops.load(Ordering::Relaxed),
        1,
        "the host's destructor ran once"
    );

    // A closure that does only host work runs to its value, and the
    // host data is the store's still.
    let value = fixture
        .store
        .run_concurrent(async |accessor| {
            accessor
                .with(|store| store.data().0.load(Ordering::Relaxed) + 41)
                .expect("the closure reaches the host data")
        })
        .await
        .expect("a closure that does only host work runs in a poisoned store");
    assert_eq!(value, 42, "the entry returns what the closure returned");
    fixture.store.data_mut().0.fetch_add(1, Ordering::Relaxed);
    assert_eq!(
        fixture.store.data().0.load(Ordering::Relaxed),
        2,
        "the host data is read and written as before"
    );
}
