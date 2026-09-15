//! Baseline tests for component composition: components that contain
//! other components and connect them through adapter modules the
//! translator emits. Cross-component calls never leave core Wasm
//! except for the intrinsics an adapter imports (string transcoders,
//! traps, resource transfer).

#![cfg(test)]

use wasm_component_model_polyfill::{Component, Engine, Linker, Store, Val};
use wcmp_macros::component;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn instantiate(bytes: &[u8]) -> (Store<()>, wasm_component_model_polyfill::Instance) {
    let engine = Engine::new().expect("engine");
    let component = Component::new(&engine, bytes).expect("component parses");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store: Store<()> = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiate");
    (store, instance)
}

#[wcmp_macros::test]
async fn it_links_two_inner_components_through_an_adapter() {
    // `$B` imports the function `$A` exports. The outer component
    // wires them together, so the call from `$B` to `$A` passes
    // through an adapter module rather than through the host.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (component $A
            (core module $m
              (func (export "double") (param i32) (result i32)
                local.get 0 i32.const 2 i32.mul))
            (core instance $i (instantiate $m))
            (func (export "double") (param "x" u32) (result u32)
              (canon lift (core func $i "double"))))
          (component $B
            (import "double" (func $double (param "x" u32) (result u32)))
            (core func $core-double (canon lower (func $double)))
            (core module $m
              (import "" "double" (func $double (param i32) (result i32)))
              (func (export "run") (param i32) (result i32)
                local.get 0 call $double i32.const 1 i32.add))
            (core instance $i (instantiate $m
              (with "" (instance (export "double" (func $core-double))))))
            (func (export "run") (param "x" u32) (result u32)
              (canon lift (core func $i "run"))))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B (with "double" (func $a "double"))))
          (export "run" (func $b "run")))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let run = instance.get_func("run").expect("run export");
    let result = run.call(&mut store, &[Val::U32(20)]).expect("call");
    assert_eq!(result.as_ref(), &[Val::U32(41)]);
}

#[wcmp_macros::test]
async fn it_copies_strings_between_inner_components_with_one_encoding() {
    // Both inner components use UTF-8. The adapter copies the string
    // from `$B`'s memory into `$A`'s memory through `$A`'s
    // `cabi_realloc`, `$A` echoes it, and the adapter copies the
    // result back into `$B`'s memory.
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
        ;; Round the bump pointer up to the requested alignment.
        global.get $bump local.get $align i32.add i32.const 1 i32.sub
        local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
        local.set $ptr
        local.get $ptr local.get $size i32.add global.set $bump
        ;; A reallocation moves the old contents into the new block.
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
        ;; Round the bump pointer up to the requested alignment.
        global.get $bump local.get $align i32.add i32.const 1 i32.sub
        local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
        local.set $ptr
        local.get $ptr local.get $size i32.add global.set $bump
        ;; A reallocation moves the old contents into the new block.
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
    let (mut store, instance) = instantiate(COMPONENT);
    let run = instance.get_func("run").expect("run export");
    let text = "hello, composed world".to_owned();
    let result = run
        .call(&mut store, &[Val::String(text.clone())])
        .expect("call");
    assert_eq!(result.as_ref(), &[Val::String(text)]);
}

#[wcmp_macros::test]
async fn it_transcodes_strings_between_inner_components() {
    // `$B` uses UTF-16 and `$A` uses UTF-8, so the adapter transcodes
    // in both directions through the polyfill's transcoder
    // intrinsics. The host side of `$B` is UTF-16 as well, which
    // also exercises the UTF-16 lift and lower at the host boundary.
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
        ;; Round the bump pointer up to the requested alignment.
        global.get $bump local.get $align i32.add i32.const 1 i32.sub
        local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
        local.set $ptr
        local.get $ptr local.get $size i32.add global.set $bump
        ;; A reallocation moves the old contents into the new block.
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
        ;; Round the bump pointer up to the requested alignment.
        global.get $bump local.get $align i32.add i32.const 1 i32.sub
        local.get $align i32.const 1 i32.sub i32.const -1 i32.xor i32.and
        local.set $ptr
        local.get $ptr local.get $size i32.add global.set $bump
        ;; A reallocation moves the old contents into the new block.
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
                (realloc (core func $i "cabi_realloc"))
            string-encoding=utf16))
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
                (realloc (core func $i "cabi_realloc"))
            string-encoding=utf16)))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B (with "echo" (func $a "echo"))))
          (export "run" (func $b "run")))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let run = instance.get_func("run").expect("run export");
    let text = "héllo wörld — ünïcødé ✓".to_owned();
    let result = run
        .call(&mut store, &[Val::String(text.clone())])
        .expect("call");
    assert_eq!(result.as_ref(), &[Val::String(text)]);
}

#[wcmp_macros::test]
async fn it_runs_the_defining_components_destructor_when_another_component_drops_the_handle() {
    // `$A` defines a resource with a destructor that counts drops and
    // exports `make`. `$B` imports the resource type and `make`,
    // creates a handle, and drops it in its own table. The drop must
    // reach `$A`'s destructor exactly once, through the adapter's
    // transfer of the owned handle between the two tables.
    const COMPONENT: &[u8] = component!(
        r#"
        (component
          (component $A
            (core module $d
              (global $drops (mut i32) (i32.const 0))
              (func (export "dtor") (param i32)
                global.get $drops i32.const 1 i32.add global.set $drops)
              (func (export "drops") (result i32) global.get $drops))
            (core instance $di (instantiate $d))
            (type $r (resource (rep i32) (dtor (core func $di "dtor"))))
            (core func $new (canon resource.new $r))
            (core module $m
              (import "" "new" (func $new (param i32) (result i32)))
              (func (export "make") (result i32) i32.const 7 call $new))
            (core instance $i (instantiate $m
              (with "" (instance (export "new" (func $new))))))
            (export $r' "r" (type $r))
            (func (export "make") (result (own $r'))
              (canon lift (core func $i "make")))
            (func (export "drops") (result u32)
              (canon lift (core func $di "drops"))))
          (component $B
            (import "r" (type $r (sub resource)))
            (import "make" (func $make (result (own $r))))
            (core func $core-make (canon lower (func $make)))
            (core func $drop (canon resource.drop $r))
            (core module $m
              (import "" "make" (func $make (result i32)))
              (import "" "drop" (func $drop (param i32)))
              (func (export "run") call $make call $drop))
            (core instance $i (instantiate $m
              (with "" (instance
                (export "make" (func $core-make))
                (export "drop" (func $drop))))))
            (func (export "run") (canon lift (core func $i "run"))))
          (instance $a (instantiate $A))
          (instance $b (instantiate $B
            (with "r" (type $a "r"))
            (with "make" (func $a "make"))))
          (export "run" (func $b "run"))
          (export "drops" (func $a "drops")))
        "#
    );
    let (mut store, instance) = instantiate(COMPONENT);
    let run = instance.get_func("run").expect("run export");
    run.call(&mut store, &[]).expect("run");
    let drops = instance.get_func("drops").expect("drops export");
    assert_eq!(
        drops.call(&mut store, &[]).expect("drops").as_ref(),
        &[Val::U32(1)],
        "the defining component's destructor ran exactly once"
    );
}
