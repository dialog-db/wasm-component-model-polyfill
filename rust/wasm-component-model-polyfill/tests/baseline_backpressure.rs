//! Baseline tests for the backpressure built-ins.
//!
//! `backpressure.inc` and `backpressure.dec` move the counter that
//! shuts one component instance's entry gate. Neither reads the
//! instance's may-leave flag, because the reference exempts them
//! both, and the test here is the case that exemption exists for: a
//! `cabi_realloc` runs while the flag is clear, and it calls both
//! built-ins.

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Instance, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

async fn instantiate(bytes: &[u8]) -> (Store<()>, Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes)
        .await
        .expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_raises_and_lowers_backpressure_from_a_realloc() {
    // The export's lift declares a `cabi_realloc`, so the string
    // argument and the string result each go through it. The realloc
    // raises and lowers the counter on every call and counts the
    // round trips. A built-in that refused to run from a realloc
    // would trap the call instead of returning the string.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core func $inc (canon backpressure.inc))
          (core func $dec (canon backpressure.dec))
          (core module $m
            (import "" "inc" (func $inc))
            (import "" "dec" (func $dec))
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 1024))
            (global $rounds (mut i32) (i32.const 0))
            (func $realloc (export "cabi_realloc")
                  (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
                  (result i32)
              (local $ptr i32)
              call $inc
              call $dec
              global.get $rounds i32.const 1 i32.add global.set $rounds
              global.get $bump local.get $align i32.add i32.const 1 i32.sub
              local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
              local.set $ptr
              local.get $ptr local.get $size i32.add global.set $bump
              local.get $ptr)
            (func (export "echo") (param i32 i32) (result i32)
              (local $ret i32)
              i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
              local.get $ret local.get 0 i32.store
              local.get $ret local.get 1 i32.store offset=4
              local.get $ret)
            (func (export "rounds") (result i32)
              global.get $rounds))
          (core instance $i (instantiate $m (with "" (instance
            (export "inc" (func $inc))
            (export "dec" (func $dec))))))
          (func (export "echo") (param "s" string) (result string)
            (canon lift (core func $i "echo")
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc"))))
          (func (export "rounds") (result u32)
            (canon lift (core func $i "rounds"))))
        "#
    );

    let (mut store, instance) = instantiate(COMPONENT).await;
    let echo = instance.get_func("echo").expect("the `echo` export");
    let rounds = instance.get_func("rounds").expect("the `rounds` export");

    let text = "backpressure".to_owned();
    let result = echo
        .call(&mut store, &[Val::String(text.clone())])
        .await
        .expect("the call returns rather than trapping");
    assert_eq!(
        result.as_ref(),
        &[Val::String(text)],
        "the call carried its string through a realloc that moved the counter"
    );

    let counted = rounds
        .call(&mut store, &[])
        .await
        .expect("the count reads back");
    let Some(Val::U32(counted)) = counted.first() else {
        panic!("the count is a `u32`");
    };
    assert!(
        *counted >= 2,
        "the argument and the result each allocated, and every allocation ran both built-ins, \
         but the realloc ran {counted} times"
    );
}
