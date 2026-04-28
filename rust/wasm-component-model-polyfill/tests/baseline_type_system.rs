//! Baseline tests for the Component Model type system as supported by
//! `wasm_component_layer` today. Each test is a stub: see PDD003's "Component
//! Type System" row. Wasip3-only valtypes (`future`, `stream`, `error-context`)
//! and subtyping live in a separate, forthcoming test file.

#![cfg(test)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_primitive_value_types() {
    todo!("round-trip every primitive (bool, s8/u8..s64/u64, f32, f64, char, string) through a component import/export");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_record_types() {
    todo!("define a component with a record-typed export and assert field access and structural shape match the WIT declaration");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_variant_types() {
    todo!("round-trip a variant value through both lift and lower paths, exercising every case including the no-payload arm");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_list_types() {
    todo!("pass a non-empty list across a component boundary and assert element ordering and length are preserved");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_option_types() {
    todo!("round-trip both `some` and `none` through an option-typed export");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_result_types() {
    todo!("round-trip both `ok` and `err` arms through a result-typed export, with and without payloads");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_tuple_types() {
    todo!("round-trip a heterogeneous tuple and assert positional access matches the declaration");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_flags_types() {
    todo!("set, clear, and round-trip individual flag bits through a flags-typed export");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_enum_types() {
    todo!("round-trip each discriminant of an enum-typed export");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_compares_types_structurally() {
    todo!("declare two components with identically-shaped but separately-defined record types and assert they unify under structural equality");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_own_resource_handles() {
    todo!("create an `own<T>` handle in a guest, transfer ownership across a host boundary, and assert subsequent guest access traps");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_supports_borrow_resource_handles() {
    todo!("borrow a resource across a host call and assert the borrow ends at function return without invalidating the owning handle");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_runs_sync_resource_destructors() {
    todo!("drop a guest-owned resource and assert the registered sync destructor runs exactly once with the expected representation");
}
