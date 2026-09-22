//! Baseline tests for what a failed instantiation leaves in the
//! store.
//!
//! An instantiation writes to the store before it can fail: one
//! instance record per component instance of the plan, then the
//! destructor and the name of every resource type the plan
//! introduces. A start function that traps and an import the
//! executor cannot look up both fail after that. The records they
//! would leave are reachable from nothing — the instance the
//! attempt would have produced does not exist — so what these tests
//! are about is hygiene: a store that took ten failed attempts
//! holds what it held before the first.
//!
//! Wasmtime's store keeps what a failed instantiation left in it, so
//! there is no parity to read here either way.

#![cfg(test)]

use crate::linker::{ImportBinding, Resolution};
use crate::store::{StoreContextInternalExt, StoreInternalExt};
use crate::{Component, Engine, Error, HostResource, InterfaceIdentifier, Linker, Store};
use wcmp_macros::component;

/// The interface the components below import their host resource
/// from.
const THINGS: &str = "test:host/things@0.1.0";

/// A component that brings in the host's resource, defines a
/// resource of its own, and instantiates one core module: an
/// instantiation of it registers a destructor and a name for two
/// resource types and adds one instance record.
const REGISTERS_RESOURCES: &[u8] = component!(
    r#"
    (component
      (import "test:host/things@0.1.0" (instance $i
        (export "thing" (type (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core func $drop-thing (canon resource.drop $thing))
      (type $r (resource (rep i32)))
      (core func $new (canon resource.new $r))
      (core module $m
        (import "host" "drop-thing" (func (param i32)))
        (import "host" "new" (func (param i32) (result i32))))
      (core instance $c (instantiate $m (with "host" (instance
        (export "drop-thing" (func $drop-thing))
        (export "new" (func $new)))))))
    "#
);

/// The same component, with a core module whose `start` function
/// traps. Everything the plan registers is in the store by the time
/// the trap happens.
const TRAPS_IN_START: &[u8] = component!(
    r#"
    (component
      (import "test:host/things@0.1.0" (instance $i
        (export "thing" (type (sub resource)))))
      (alias export $i "thing" (type $thing))
      (core func $drop-thing (canon resource.drop $thing))
      (type $r (resource (rep i32)))
      (core func $new (canon resource.new $r))
      (core module $m
        (import "host" "drop-thing" (func (param i32)))
        (import "host" "new" (func (param i32) (result i32)))
        (func $boom unreachable)
        (start $boom))
      (core instance $c (instantiate $m (with "host" (instance
        (export "drop-thing" (func $drop-thing))
        (export "new" (func $new)))))))
    "#
);

/// A component that imports nothing and keeps no resource. An
/// instantiation of it still sweeps the linker for the labels of the
/// host resources it carries.
const QUIET: &[u8] = component!(
    r#"
    (component
      (core module $m (func (export "noop")))
      (core instance $c (instantiate $m)))
    "#
);

/// What the store holds of the records an instantiation adds: one
/// per component instance of the plan, the destructors its resource
/// types registered, and the names it taught for them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Records {
    instances: usize,
    destructors: usize,
    names: usize,
}

/// Read those records out of the store.
fn records<T: 'static>(store: &mut Store<T>) -> Records {
    let instances = store
        .internal()
        .tables()
        .lock()
        .expect("handle tables")
        .tasks
        .instances()
        .len();
    let context = store.internal().context();
    Records {
        instances,
        destructors: context.internal_ref().registered_destructors(),
        names: context.internal_ref().learned_resource_names(),
    }
}

/// A linker holding the host resource the components import, under
/// the label they import it by.
fn things_linker(engine: &Engine, resource: HostResource<()>) -> Linker<()> {
    let mut linker: Linker<()> = Linker::new(engine);
    let iface: InterfaceIdentifier = THINGS.parse().expect("identifier");
    linker
        .instance(&iface)
        .resource_with("thing", resource)
        .expect("the registration");
    linker
}

/// The host resource the components import, with a destructor that
/// does nothing.
fn thing() -> HostResource<()> {
    HostResource::new(|_: &mut (), _: u32| -> crate::Result<()> { Ok(()) })
}

#[wcmp_macros::test]
async fn it_leaves_the_store_as_it_found_it_when_a_start_function_traps() {
    // The plan registers both of its resource types and reserves its
    // instance record before the core module whose `start` function
    // traps is instantiated, so every record the attempt added is in
    // the store when the trap comes back.
    let engine = Engine::new().expect("engine");
    let works = Component::new(&engine, REGISTERS_RESOURCES)
        .await
        .expect("component parses");
    let traps = Component::new(&engine, TRAPS_IN_START)
        .await
        .expect("component parses");
    let linker = things_linker(&engine, thing());
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    let _instance = linker
        .instantiate(&mut store, &works)
        .await
        .expect("the component that does not trap instantiates");
    let before = records(&mut store);
    assert!(
        before.instances > 0 && before.destructors > 0 && before.names > 0,
        "the first instantiation left records for the second to leave alone, got {before:?}"
    );

    let error = match linker.instantiate(&mut store, &traps).await {
        Ok(_) => panic!("a start function that traps fails the instantiation"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Instantiation(_)),
        "the trapping start function is what failed, got {error:?}"
    );

    assert_eq!(
        records(&mut store),
        before,
        "the failed attempt left the store as it found it, and failed with {error}"
    );
}

#[wcmp_macros::test]
async fn it_leaves_the_store_as_it_found_it_when_a_resource_runtime_does_not_link() {
    // The executor looks the host resource up for itself as it walks
    // the plan, and that lookup fails before the plan has registered
    // anything — with the instance records already reserved. The
    // resolver would not hand it a resolution that misses, so the
    // executor is driven here with one that names a root entry the
    // linker does not hold.
    let engine = Engine::new().expect("engine");
    let works = Component::new(&engine, REGISTERS_RESOURCES)
        .await
        .expect("component parses");
    let linker = things_linker(&engine, thing());
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    let _instance = linker
        .instantiate(&mut store, &works)
        .await
        .expect("the component instantiates through its own resolution");
    let before = records(&mut store);

    let resolution = Resolution {
        bindings: vec![ImportBinding::Root {
            name: "nothing-is-registered-here".to_owned(),
        }],
    };
    let mut context = store.internal().context();
    let error = match crate::executor::instantiate(&works, &mut context, &linker, &resolution) {
        Ok(_) => panic!("the resource import resolves to no registration"),
        Err(error) => error,
    };
    drop(context);
    assert!(
        matches!(error, Error::Link(_)),
        "the resource runtime is what failed to link, got {error:?}"
    );

    assert_eq!(
        records(&mut store),
        before,
        "the failed attempt left the store as it found it, and failed with {error}"
    );
}

#[wcmp_macros::test]
async fn it_puts_back_the_resource_name_a_failed_instantiation_displaced() {
    // One host resource registered under two labels is one identity
    // under two names. An instantiation that succeeds first leaves
    // the label no component uses as the store's fallback; the
    // attempt that traps teaches the label its own component
    // imports, which outranks a fallback. Taking the attempt's
    // registrations back has to put the tier back with the label, or
    // the store would render nothing for the identity where it
    // rendered the fallback before.
    let engine = Engine::new().expect("engine");
    let quiet = Component::new(&engine, QUIET)
        .await
        .expect("component parses");
    let traps = Component::new(&engine, TRAPS_IN_START)
        .await
        .expect("component parses");
    let resource = thing();
    let mut linker = things_linker(&engine, resource.clone());
    let type_id = linker
        .root()
        .resource_with("alias", resource)
        .expect("the registration");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    let _instance = linker
        .instantiate(&mut store, &quiet)
        .await
        .expect("the quiet component instantiates");
    let before = records(&mut store);
    let swept = store
        .internal()
        .context()
        .internal_ref()
        .resource_type(type_id)
        .expect("the sweep left a label for the identity no component imported");
    assert_eq!(
        swept.label(),
        "alias",
        "the sweep takes the labels in sorted order and `alias` sorts first"
    );

    let error = match linker.instantiate(&mut store, &traps).await {
        Ok(_) => panic!("a start function that traps fails the instantiation"),
        Err(error) => error,
    };

    let after = store
        .internal()
        .context()
        .internal_ref()
        .resource_type(type_id)
        .expect("the identity still has the label it had before the attempt");
    assert_eq!(
        after.label(),
        "alias",
        "the attempt taught `thing` and took it back, and failed with {error}"
    );
    assert_eq!(
        records(&mut store),
        before,
        "the failed attempt left the store as it found it"
    );
}
