//! Baseline tests for component binary parsing — the surface
//! `wasm_component_layer` already covers and the polyfill must continue to
//! provide. Each test is a stub: see PDD003's "Component Binary Format" row.

#![cfg(test)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_parses_the_component_preamble() {
    todo!("load a minimal component binary and assert the polyfill accepts the `\\0asm` + component version word");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_decodes_top_level_component_sections() {
    todo!("walk a component binary that exercises type, import, core-module, instance, alias, and export sections; assert each is reachable through the polyfill's parsed representation");
}

#[wcmp_macros::test]
#[ignore = "stub: polyfill implementation pending"]
async fn it_rejects_a_malformed_component_binary() {
    todo!("feed a binary with a corrupted preamble (or truncated section) into the polyfill and assert a structured parse error is returned, not a panic");
}
