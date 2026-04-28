//! Smoke tests proving that the three `wcmp_macros` macros compose: an async
//! cross-target `#[test]` that asserts on byte slices produced by `wasm!` and
//! `component!` at compile time.

#![cfg(test)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const ADDER: &[u8] = wcmp_macros::wasm!(
    r#"
    (module
      (func (export "add") (param i32 i32) (result i32)
        local.get 0 local.get 1 i32.add))
    "#
);

const TRIVIAL_COMPONENT: &[u8] = wcmp_macros::component!(
    r#"
    (component
      (core module $m
        (func (export "f") (result i32) i32.const 42))
      (core instance $i (instantiate $m))
      (func (export "f") (canon lift (core func $i "f"))))
    "#
);

#[wcmp_macros::test]
async fn wasm_macro_emits_core_module_header() {
    assert_eq!(&ADDER[..4], b"\0asm", "core module preamble");
    assert_eq!(&ADDER[4..8], &[0x01, 0x00, 0x00, 0x00], "core version word");
}

#[wcmp_macros::test]
async fn component_macro_emits_component_header() {
    assert_eq!(&TRIVIAL_COMPONENT[..4], b"\0asm", "component preamble");
    assert_eq!(
        &TRIVIAL_COMPONENT[4..8],
        &[0x0d, 0x00, 0x01, 0x00],
        "component version word",
    );
}
