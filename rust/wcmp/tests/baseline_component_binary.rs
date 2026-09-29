//! Baseline tests for component binary parsing — the surface
//! `wasm_component_layer` already covers and the polyfill must continue to
//! provide.

#![cfg(test)]

use wcmp::{Component, Engine, EngineConfig, Error, ExternType, ExternalName, Linker, Store};
use wcmp_macros::{component, wasm};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const EMPTY_COMPONENT: &[u8] = component!("(component)");

const SECTION_RICH_COMPONENT: &[u8] = component!(
    r#"
    (component
      (type $iface (instance
        (export "greet" (func (param "who" string) (result string)))))
      (import "wasi:cli/run@0.2.0" (instance $i (type $iface)))
      (core module $m
        (func (export "f")))
      (core instance $core_i (instantiate $m))
      (alias core export $core_i "f" (core func $core_f))
      (func (export "do-it") (canon lift (core func $core_f))))
    "#
);

const CORE_MODULE: &[u8] = wasm!("(module)");

#[wcmp_macros::test]
async fn it_parses_the_component_preamble() {
    let engine = Engine::new().expect("engine constructs");
    let component = Component::new(&engine, EMPTY_COMPONENT)
        .await
        .expect("empty component parses");

    assert!(component.imports.is_empty(), "no imports declared");
    assert!(component.exports.is_empty(), "no exports declared");
}

#[wcmp_macros::test]
async fn it_decodes_top_level_component_sections() {
    let engine = Engine::new().expect("engine constructs");
    let component = Component::new(&engine, SECTION_RICH_COMPONENT)
        .await
        .expect("rich component parses");

    // The fixture exercises every section the test is meant to cover:
    // a component-type section (the `$greet` function type), an
    // import section (the `wasi:cli/run` function), a core-module
    // section (the nested `$m`), a core-instance section (`$i`), an
    // alias section (the core-func alias), a canonical section (the
    // canon lift), and an export section (`f`). Each section's effect
    // is verifiable through the polyfill's parsed view: a successful
    // decode means every section was read, and the resulting imports
    // and exports witness the import / type / export work.
    assert_eq!(component.imports.len(), 1, "one import declared");
    assert_eq!(component.exports.len(), 1, "one export declared");

    // The import is the interface-named instance from the type
    // section, and its declared shape round-trips through the
    // polyfill's data shapes.
    let import = &component.imports[0];
    assert!(
        matches!(import.name, ExternalName::Interface(_)),
        "import is an interface name"
    );
    let instance = match &import.ty {
        ExternType::Instance(i) => i,
        other => panic!("expected the import to be an instance, got {other:?}"),
    };
    assert_eq!(instance.items.len(), 1, "interface contains one item");
    assert_eq!(instance.items[0].name, "greet");
    assert!(matches!(instance.items[0].ty, ExternType::Function(_)));

    // The export is the canon-lifted function — a polyfill-side
    // function, witnessing that the alias and canonical sections
    // were processed.
    let export = &component.exports[0];
    assert_eq!(export.name, ExternalName::Plain("do-it".to_owned()));
    assert!(
        matches!(export.ty, ExternType::Function(_)),
        "export is a function"
    );
}

#[wcmp_macros::test]
async fn it_rejects_a_malformed_component_binary() {
    let engine = Engine::new().expect("engine constructs");

    // A truncated preamble: magic bytes plus a single byte of the
    // version word, leaving the rest unread.
    let truncated: [u8; 5] = [b'\0', b'a', b's', b'm', 0x0d];
    let truncation = Component::new(&engine, &truncated)
        .await
        .expect_err("truncated bytes are rejected with a structured error");
    assert!(matches!(truncation, Error::InvalidComponentBinary { .. }));

    // A core module masquerading as a component: a valid Wasm
    // binary, but not a component.
    let wrong_kind = Component::new(&engine, CORE_MODULE)
        .await
        .expect_err("core module bytes are rejected as not-a-component");
    assert!(matches!(wrong_kind, Error::NotAComponent));
}

/// One mebibyte of data in a core module: far beyond the size at
/// which Chrome refuses a synchronous `WebAssembly.Module` on the
/// main thread, so the browser lane proves the compile goes through
/// the asynchronous path.
const LARGE_PAYLOAD: usize = 1 << 20;

fn large_component() -> Vec<u8> {
    let payload = "A".repeat(LARGE_PAYLOAD);
    let wat = format!(
        r#"(component
          (core module $m
            (memory 17)
            (data (i32.const 0) "{payload}")
            (func (export "size") (result i32) i32.const {LARGE_PAYLOAD}))
          (core instance $i (instantiate $m))
          (func (export "size") (result u32) (canon lift (core func $i "size"))))"#
    );
    let buffer = wast::parser::ParseBuffer::new(&wat).expect("lex the component");
    let mut wat = wast::parser::parse::<wast::Wat>(&buffer).expect("parse the component");
    wat.encode().expect("encode the component")
}

#[wcmp_macros::test]
async fn it_loads_a_component_larger_than_the_synchronous_compile_limit() {
    let engine = Engine::new().expect("engine");
    let bytes = large_component();
    assert!(
        bytes.len() > LARGE_PAYLOAD,
        "the binary carries the payload"
    );
    let component = Component::new(&engine, &bytes)
        .await
        .expect("compile the component");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate the component");
    let size = instance
        .get_func("size")
        .expect("size export")
        .typed::<(), u32>()
        .expect("typed size");
    assert_eq!(
        size.call(&mut store, ()).await.expect("call size"),
        LARGE_PAYLOAD as u32
    );
}

/// Nine mebibytes of data in a core module: beyond the 8 MB above
/// which Chrome refuses a synchronous `WebAssembly.Module` on the
/// main thread. Chrome refuses a synchronous `WebAssembly.Instance`
/// above the same size, so a module this large compiles in the
/// browser but does not instantiate there.
const DUPLICATE_PAYLOAD: usize = 9 << 20;

/// A component with two byte-identical core modules of `payload`
/// bytes of data each, each instantiated once and exported through
/// its own function. The modules carry no identifier, which the text
/// format would record in a name section and so tell them apart. Each
/// holds a counter, so a call shows which instance it reached.
fn duplicate_module_component(payload: usize) -> Vec<u8> {
    let pages = payload.div_ceil(1 << 16).max(1);
    let payload = "A".repeat(payload);
    let module = format!(
        r#"(core module
            (memory {pages})
            (data (i32.const 0) "{payload}")
            (global $count (mut i32) (i32.const 0))
            (func (export "bump") (result i32)
              global.get $count
              i32.const 1
              i32.add
              global.set $count
              global.get $count))"#
    );
    let wat = format!(
        r#"(component
          {module}
          {module}
          (core instance $first (instantiate 0))
          (core instance $second (instantiate 1))
          (func (export "first") (result u32) (canon lift (core func $first "bump")))
          (func (export "second") (result u32) (canon lift (core func $second "bump"))))"#
    );
    let buffer = wast::parser::ParseBuffer::new(&wat).expect("lex the component");
    let mut wat = wast::parser::parse::<wast::Wat>(&buffer).expect("parse the component");
    wat.encode().expect("encode the component")
}

#[wcmp_macros::test]
async fn it_compiles_byte_identical_core_modules_above_the_synchronous_limit() {
    // The browser's asynchronous compile leaves one entry per byte
    // string on the engine, and a module built without one compiles
    // synchronously, which the browser refuses at this size. The
    // component compiling is what shows neither module did.
    let engine = Engine::new().expect("engine");
    let bytes = duplicate_module_component(DUPLICATE_PAYLOAD);
    assert!(
        bytes.len() > 2 * DUPLICATE_PAYLOAD,
        "the binary carries both payloads"
    );
    Component::new(&engine, &bytes)
        .await
        .expect("compile the component");
}

#[wcmp_macros::test]
async fn it_instantiates_each_of_two_byte_identical_core_modules() {
    // The two modules share one compile, and each instantiation of it
    // is its own: calls through one export leave the other's counter
    // where it was.
    let engine = Engine::new().expect("engine");
    let bytes = duplicate_module_component(64);
    let component = Component::new(&engine, &bytes)
        .await
        .expect("compile the component");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = linker
        .instantiate(&mut store, &component)
        .await
        .expect("instantiate the component");
    let first = instance
        .get_func("first")
        .expect("first export")
        .typed::<(), u32>()
        .expect("typed first");
    let second = instance
        .get_func("second")
        .expect("second export")
        .typed::<(), u32>()
        .expect("typed second");
    assert_eq!(first.call(&mut store, ()).await.expect("call first"), 1);
    assert_eq!(first.call(&mut store, ()).await.expect("call first"), 2);
    assert_eq!(
        second.call(&mut store, ()).await.expect("call second"),
        1,
        "the second module's instance keeps its own counter"
    );
}

/// The futures the entry points return are `Send` on native, so a
/// host can drive them from a multi-threaded runtime. The browser
/// has no threads to send them to, so the check is native-only.
#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
async fn it_returns_send_futures_on_native() {
    fn assert_send<T: Send>(future: T) -> T {
        future
    }
    let engine = Engine::new().expect("engine");
    let component = assert_send(Component::new(&engine, EMPTY_COMPONENT))
        .await
        .expect("compile the component");
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, ()).expect("store");
    let instance = assert_send(linker.instantiate(&mut store, &component))
        .await
        .expect("instantiate the component");
    assert!(instance.exports().func("f").is_none());
}

#[wcmp_macros::test]
async fn it_validates_with_wasmtimes_feature_gates() {
    // The `implements` annotation is gated in Wasmtime, so the
    // default engine rejects it and an engine that opts in accepts
    // it.
    const IMPLEMENTS: &[u8] = component!(
        r#"
        (component (import "a" (implements "a:b/c") (instance)))
        "#
    );
    let engine = Engine::new().expect("engine");
    let err = Component::new(&engine, IMPLEMENTS)
        .await
        .expect_err("`implements` is off by default");
    match err {
        Error::InvalidComponentBinary { message, .. } => {
            assert!(
                message.contains("cm-implements"),
                "the rejection names the gate: {message}"
            );
        }
        other => panic!("expected a validation failure, got {other:?}"),
    }

    let mut config = EngineConfig::new();
    config.wasm_component_model_implements(true);
    let engine = Engine::with_config(&config).expect("engine");
    Component::new(&engine, IMPLEMENTS)
        .await
        .expect("an engine that opts in accepts `implements`");

    // Nested namespaces and projections in extern names are not a
    // feature Wasmtime enables at all, so no configuration accepts
    // them.
    const NESTED: &[u8] = component!(
        r#"
        (component (import "foo:bar/baz/qux" (func)))
        "#
    );
    let err = Component::new(&engine, NESTED)
        .await
        .expect_err("a nested projection is rejected");
    assert!(matches!(err, Error::InvalidComponentBinary { .. }));
}
