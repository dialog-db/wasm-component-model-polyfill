//! Only the boundary of a module is the runtime layer's to describe.

use wcmp_macros::wasm;
use wcmp_wasm_core::{Capability, Engine, ExportType, ExternType, FuncType, TagType, Val, ValType};

use crate::support;

/// A module shaped as the Zena compiler emits one for a program that uses
/// exceptions: a mutable global of a GC reference type, and an exported
/// tag. The component around such a module aliases neither.
const ZENA_SHAPED: &[u8] = wasm!(
    r#"
    (module
      (type $box (struct (field (mut i32))))
      (global $state (mut (ref null $box)) (ref.null $box))
      (tag $exception (export "__zena_exception") (param i32))
      (func (export "bump") (result i32)
        global.get $state
        ref.is_null
        if
          i32.const 0
          struct.new $box
          global.set $state
        end
        global.get $state
        global.get $state
        struct.get $box 0
        i32.const 1
        i32.add
        struct.set $box 0
        global.get $state
        struct.get $box 0))
    "#
);

/// A module that defines a global of a GC reference type and exports a
/// tag instantiates, and its exported function runs, on every backend that
/// declares `gc` and `exceptions`.
pub async fn it_instantiates_a_module_with_a_gc_global_and_an_exported_tag(engine: &Engine) {
    if !support::declares(engine, &[Capability::Gc, Capability::Exceptions]) {
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, ZENA_SHAPED, &[]).await;
    let module = support::module(engine, ZENA_SHAPED).await;
    assert!(
        module.exports().any(|export| export
            == &ExportType::new(
                "__zena_exception",
                ExternType::Tag(TagType::new(FuncType::new([ValType::I32], []))),
            )),
        "the module exports its tag: {module:?}"
    );

    let bump = support::func(&mut store, instance, "bump");
    for expected in [1, 2, 3] {
        let count = support::call(&mut store, bump, &[], &[ValType::I32]);
        assert_eq!(count[0].i32(), Some(expected));
    }
}

/// A module whose table of a concrete reference type, tag, and GC global
/// are all internal, and which exports one function, loads and runs. None
/// of its internal items is a reason to refuse it.
pub async fn it_loads_a_module_whose_internal_items_do_not_cross_its_boundary(engine: &Engine) {
    let needs = [
        Capability::Gc,
        Capability::Exceptions,
        Capability::FunctionReferences,
    ];
    if !support::declares(engine, &needs) {
        return;
    }
    let bytes = wasm!(
        r#"
        (module
          (type $node (struct (field i32)))
          (type $callback (func (param i32) (result i32)))
          (table $callbacks 1 (ref null $callback))
          (tag $internal (param (ref null $node)))
          (global $root (mut (ref null $node)) (ref.null $node))
          (func $double (type $callback)
            local.get 0
            i32.const 2
            i32.mul)
          (elem declare func $double)
          (func (export "run") (param i32) (result i32)
            i32.const 0
            ref.func $double
            table.set $callbacks
            local.get 0
            struct.new $node
            global.set $root
            block $caught (result (ref null $node))
              try_table (catch $internal $caught)
                global.get $root
                throw $internal
              end
              unreachable
            end
            struct.get $node 0
            i32.const 0
            call_indirect $callbacks (type $callback)))
        "#
    );

    let module = support::module(engine, bytes).await;
    assert_eq!(module.imports().len(), 0);
    assert_eq!(
        module.exports().cloned().collect::<Vec<_>>(),
        [ExportType::new(
            "run",
            ExternType::Func(FuncType::new([ValType::I32], [ValType::I32])),
        )]
    );

    let mut store = support::store(engine, ());
    let instance = support::instance(&mut store, bytes, &[]).await;
    let run = support::func(&mut store, instance, "run");
    let doubled = support::call(&mut store, run, &[Val::I32(21)], &[ValType::I32]);
    assert_eq!(doubled[0].i32(), Some(42));
}
