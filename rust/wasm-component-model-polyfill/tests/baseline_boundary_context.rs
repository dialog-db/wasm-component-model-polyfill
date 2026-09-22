//! Baseline tests for the boundary context: the one lift and lower
//! crossing.
//!
//! A value crosses between the host's `Val` and a guest through one
//! boundary context, built from the canon options of the lift or
//! lower, the component instance, and the task or subtask the
//! crossing counts against. The context is the only object that
//! reads guest memory, writes guest memory, or asks the guest for
//! memory.
//!
//! There are three call sites that build one, and one test here
//! exercises each: an export call, a host trampoline, and an
//! adapter's transcoder. Each component below declares canon options
//! the crossing cannot get right by accident — a string encoding
//! that is not the default, a `cabi_realloc` the value does not fit
//! without, and a `post-return` — so a crossing that did not carry
//! its options would be visible in the result.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use wasm_component_model_polyfill::{
    Component, Engine, HostCall, Instance, Linker, Result, Store, Val,
};
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
async fn it_crosses_an_export_call_through_one_boundary_context() {
    // The export's lift declares a memory, a `cabi_realloc`, a
    // `post-return`, and the UTF-16 string encoding. `Func::call`
    // builds one context from those options for the arguments and
    // one for the result. A crossing that carried the wrong encoding
    // would return a different string, and one that never reached
    // the options' `post-return` would leave the counter at zero.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 1024))
            (global $returns (mut i32) (i32.const 0))
            (func $realloc (export "cabi_realloc")
                  (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
                  (result i32)
              (local $ptr i32)
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
            (func (export "after") (param i32)
              global.get $returns i32.const 1 i32.add global.set $returns)
            (func (export "returns") (result i32)
              global.get $returns))
          (core instance $i (instantiate $m))
          (func (export "echo") (param "s" string) (result string)
            (canon lift (core func $i "echo")
                       string-encoding=utf16
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc"))
                       (post-return (core func $i "after"))))
          (func (export "returns") (result u32)
            (canon lift (core func $i "returns"))))
        "#
    );

    let (mut store, instance) = instantiate(COMPONENT).await;
    let echo = instance.get_func("echo").expect("echo export");
    let returns = instance.get_func("returns").expect("returns export");

    // `café` is four UTF-16 code units and five UTF-8 bytes, so the
    // round trip only reproduces it when both crossings read the
    // encoding off the context's options.
    let text = "café".to_owned();
    let result = echo
        .call(&mut store, &[Val::String(text.clone())])
        .await
        .expect("call");
    assert_eq!(result.as_ref(), &[Val::String(text)]);

    let after = returns.call(&mut store, &[]).await.expect("returns");
    assert_eq!(
        after.as_ref(),
        &[Val::U32(1)],
        "the context ran the post-return its options name, exactly once"
    );
}

#[wcmp_macros::test]
async fn it_crosses_a_host_trampoline_through_one_boundary_context() {
    // The guest calls a host import whose lowering declares the
    // memory, the `cabi_realloc`, and the UTF-16 encoding of the
    // caller. The trampoline builds one context to lift the
    // parameters out of the guest and one to lower the host's result
    // back in, and the string the host observes says whether both
    // carried the lowering's options.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (import "shout" (func $shout (param "s" string) (result string)))
          (core module $shim
            (table (export "$imports") 1 1 funcref)
            (func (export "0") (param i32 i32 i32)
              local.get 0 local.get 1 local.get 2
              i32.const 0
              call_indirect (param i32 i32 i32)))
          (core instance $shim (instantiate $shim))
          (core module $m
            (import "" "shout" (func $shout (param i32 i32 i32)))
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 1024))
            (func $realloc (export "cabi_realloc")
                  (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
                  (result i32)
              (local $ptr i32)
              global.get $bump local.get $align i32.add i32.const 1 i32.sub
              local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
              local.set $ptr
              local.get $ptr local.get $size i32.add global.set $bump
              local.get $old
              i32.eqz
              if
                local.get $ptr
                return
              end
              local.get $ptr
              local.get $old
              local.get $old-size local.get $size
              local.get $old-size local.get $size i32.lt_u
              select
              memory.copy
              local.get $ptr)
            (func (export "run") (param i32 i32) (result i32)
              (local $ret i32)
              i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
              local.get 0 local.get 1 local.get $ret call $shout
              local.get $ret))
          (core instance $i (instantiate $m
            (with "" (instance (export "shout" (func $shim "0"))))))
          (core func $core-shout
            (canon lower (func $shout)
                        string-encoding=utf16
                        (memory (core memory $i "memory"))
                        (realloc (core func $i "cabi_realloc"))))
          (core module $fixups
            (import "" "0" (func (param i32 i32 i32)))
            (import "" "$imports" (table 1 1 funcref))
            (elem (i32.const 0) func 0))
          (core instance (instantiate $fixups
            (with "" (instance
              (export "0" (func $core-shout))
              (export "$imports" (table $shim "$imports"))))))
          (func (export "run") (param "s" string) (result string)
            (canon lift (core func $i "run")
                       string-encoding=utf16
                       (memory (core memory $i "memory"))
                       (realloc (core func $i "cabi_realloc")))))
        "#
    );

    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, COMPONENT)
        .await
        .expect("component parses");
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");

    // What the host saw of the parameter the trampoline lifted.
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let recorded = seen.clone();
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .root()
        .func_wrap(
            "shout",
            move |_: HostCall<'_, ()>, (s,): (String,)| -> Result<String> {
                *recorded.lock().expect("record") = Some(s.clone());
                Ok(s.to_uppercase())
            },
        )
        .expect("the registration");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate");

    let run = instance.get_func("run").expect("run export");
    let result = run
        .call(&mut store, &[Val::String("crème".to_owned())])
        .await
        .expect("call");

    assert_eq!(
        seen.lock().expect("record").as_deref(),
        Some("crème"),
        "the lift context read the parameter under the lowering's options"
    );
    assert_eq!(
        result.as_ref(),
        &[Val::String("CRÈME".to_owned())],
        "the lower context wrote the host's result back through them"
    );
}

#[wcmp_macros::test]
async fn it_crosses_an_adapter_transcode_through_one_boundary_context() {
    // `$B` lifts and lowers in UTF-8 and `$A` in `latin1+utf16`, so
    // the call between them goes through an adapter whose transcoder
    // converts in both directions. The transcoder is one crossing
    // between two guest memories: it builds one context for the copy
    // and names neither memory itself.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (component $A
            (core module $m
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))
              (func $realloc (export "cabi_realloc")
                    (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
                    (result i32)
                (local $ptr i32)
                global.get $bump local.get $align i32.add i32.const 1 i32.sub
                local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
                local.set $ptr
                local.get $ptr local.get $size i32.add global.set $bump
                local.get $old
                i32.eqz
                if
                  local.get $ptr
                  return
                end
                local.get $ptr
                local.get $old
                local.get $old-size local.get $size
                local.get $old-size local.get $size i32.lt_u
                select
                memory.copy
                local.get $ptr)
              (func (export "echo") (param i32 i32) (result i32)
                (local $ret i32)
                i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
                local.get $ret local.get 0 i32.store
                local.get $ret local.get 1 i32.store offset=4
                local.get $ret))
            (core instance $i (instantiate $m))
            (func (export "echo") (param "s" string) (result string)
              (canon lift (core func $i "echo")
                         string-encoding=latin1+utf16
                         (memory (core memory $i "memory"))
                         (realloc (core func $i "cabi_realloc")))))
          (component $B
            (import "echo" (func $echo (param "s" string) (result string)))
            (core module $shim
              (table (export "$imports") 1 1 funcref)
              (func (export "0") (param i32 i32 i32)
                local.get 0 local.get 1 local.get 2
                i32.const 0
                call_indirect (param i32 i32 i32)))
            (core instance $shim (instantiate $shim))
            (core module $m
              (import "" "echo" (func $echo (param i32 i32 i32)))
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))
              (func $realloc (export "cabi_realloc")
                    (param $old i32) (param $old-size i32) (param $align i32) (param $size i32)
                    (result i32)
                (local $ptr i32)
                global.get $bump local.get $align i32.add i32.const 1 i32.sub
                local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
                local.set $ptr
                local.get $ptr local.get $size i32.add global.set $bump
                local.get $old
                i32.eqz
                if
                  local.get $ptr
                  return
                end
                local.get $ptr
                local.get $old
                local.get $old-size local.get $size
                local.get $old-size local.get $size i32.lt_u
                select
                memory.copy
                local.get $ptr)
              (func (export "run") (param i32 i32) (result i32)
                (local $ret i32)
                i32.const 0 i32.const 0 i32.const 4 i32.const 8 call $realloc local.set $ret
                local.get 0 local.get 1 local.get $ret call $echo
                local.get $ret))
            (core instance $i (instantiate $m
              (with "" (instance (export "echo" (func $shim "0"))))))
            (core func $core-echo
              (canon lower (func $echo)
                          (memory (core memory $i "memory"))
                          (realloc (core func $i "cabi_realloc"))))
            (core module $fixups
              (import "" "0" (func (param i32 i32 i32)))
              (import "" "$imports" (table 1 1 funcref))
              (elem (i32.const 0) func 0))
            (core instance (instantiate $fixups
              (with "" (instance
                (export "0" (func $core-echo))
                (export "$imports" (table $shim "$imports"))))))
            (func (export "run") (param "s" string) (result string)
              (canon lift (core func $i "run")
                         (memory (core memory $i "memory"))
                         (realloc (core func $i "cabi_realloc")))))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B (with "echo" (func $a "echo"))))
          (export "run" (func $b "run")))
        "#
    );

    let (mut store, instance) = instantiate(COMPONENT).await;
    let run = instance.get_func("run").expect("run export");

    // Latin-1 on the way in and out: every scalar fits in one byte.
    let latin1 = "façade".to_owned();
    let result = run
        .call(&mut store, &[Val::String(latin1.clone())])
        .await
        .expect("a latin-1 round trip");
    assert_eq!(result.as_ref(), &[Val::String(latin1)]);

    // A scalar above U+00FF forces the UTF-16 representation, so the
    // copy takes the other transcoder of the same crossing.
    let wide = "façade ♥".to_owned();
    let result = run
        .call(&mut store, &[Val::String(wide.clone())])
        .await
        .expect("a utf-16 round trip");
    assert_eq!(result.as_ref(), &[Val::String(wide)]);
}
