//! Baseline tests for linking, instantiation, and host integration as
//! `wasm_component_layer` supports them today. Each test is a stub: see
//! PDD003's "Linking, Instantiation, and Host Integration" row. Async host
//! functions, async resource destructors, host-binding code generation, and
//! component-level `start` live in a separate, forthcoming test file.

#![cfg(test)]

use wasm_component_model_polyfill as wcmp;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
async fn it_constructs_an_engine() {
    let engine = wcmp::Engine::new().expect("engine construction succeeds");

    // Engines are advertised as cheap to clone in PDD005; exercise that.
    let _clone = engine.clone();
}

#[wcmp_macros::test]
async fn it_constructs_a_store() {
    let engine = wcmp::Engine::new().expect("engine construction succeeds");

    // Construct against an engine; confirm the host-data slot is reachable
    // through `data` and `data_mut`.
    let mut store: wcmp::Store<u32> =
        wcmp::Store::new(&engine, 7).expect("store construction succeeds");
    assert_eq!(*store.data(), 7);

    *store.data_mut() = 42;
    assert_eq!(*store.data(), 42);
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_loads_a_component_from_bytes() {
    todo!(
        "parse a known-good component binary into a `Component` value and assert its declared imports and exports are introspectable"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_instantiates_a_component_through_a_linker() {
    todo!(
        "link a component with a `Linker`, instantiate it, and call an exported function returning a primitive"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_multiple_independent_instances() {
    todo!(
        "instantiate the same `Component` twice in the same `Store` and assert their state is isolated"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_resolves_package_and_interface_identifiers_with_semver() {
    todo!(
        "link a component whose imports are qualified by `PackageName` and `InterfaceIdentifier` (including a semver constraint) and assert resolution succeeds"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_defines_an_untyped_host_function() {
    todo!(
        "register a host function via the polyfill's untyped (`Val`-based) API, call it from a guest, and assert the values round-trip"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_defines_a_typed_host_function() {
    todo!(
        "register a host function via the polyfill's typed `func_wrap` equivalent and assert argument/return types are checked at link time"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_defines_a_host_resource_with_a_sync_destructor() {
    todo!(
        "declare a host-owned `ResourceType`, hand a handle to a guest, drop it, and assert the host destructor observes the drop synchronously"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_invokes_an_exported_component_function() {
    todo!(
        "end-to-end: load → link → instantiate → call → assert; the canonical happy-path smoke test for the synchronous baseline"
    );
}
