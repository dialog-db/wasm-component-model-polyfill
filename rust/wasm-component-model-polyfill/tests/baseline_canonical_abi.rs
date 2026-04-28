//! Baseline tests for the Canonical ABI surface that `wasm_component_layer`
//! already exercises. Each test is a stub: see PDD003's "Canonical ABI" row.
//! Async lift/lower, per-task context threading, and the generalized handle
//! table extension to futures and streams live in a separate, forthcoming
//! test file.

#![cfg(test)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_lifts_and_lowers_every_value_type() {
    todo!(
        "call a component export whose signature exercises every valtype in both argument and result position, asserting bit-exact round-tripping"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_invokes_cabi_realloc_during_lowering() {
    todo!(
        "lower a host-provided heap-allocating value (e.g. a string) into guest memory and assert the guest's `cabi_realloc` is called with the expected size and alignment"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_lifts_and_lowers_specialized_list_types() {
    todo!(
        "round-trip `list<u8>` and `list<u32>` payloads and assert the results match the source bytes; the polyfill is free to use specialized fast paths so long as observable behaviour is preserved"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_invokes_post_return_after_a_sync_lift() {
    todo!(
        "call a sync-lifted export whose component declares a `post-return` and assert it runs after the caller observes the return value"
    );
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_tracks_resource_handles_in_a_handle_table() {
    todo!(
        "allocate multiple resource handles, drop one, allocate another, and assert handle indices behave per the runtime-state rules in the canonical ABI (no aliasing, deterministic reuse semantics)"
    );
}
