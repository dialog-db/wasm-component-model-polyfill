//! What the browser backend holds beyond the backend contract: the probes
//! of its capabilities, a module above the browser's limit for a
//! synchronous compile, linking by the objects of the browser, and the
//! externs the browser makes.

#![cfg(target_arch = "wasm32")]

use js_sys::Reflect;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wcmp_macros::wasm;
use wcmp_wasm_core::backend::RawHandle;
use wcmp_wasm_core::{
    Capabilities, Capability, Engine, Error, Extern, Func, Global, GlobalType, Instance, Memory,
    MemoryType, Module, Mutability, RefType, Store, Table, TableType, Val, ValType,
};
use wcmp_wasm_core_web::Web;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

/// The browser's limit for a synchronous compile on the main thread.
const SYNCHRONOUS_LIMIT: usize = 8 * 1024 * 1024;

/// The module `bytes`, instantiated in a new store of `engine`.
async fn instance(engine: &Engine, bytes: &[u8]) -> (Store<()>, Instance) {
    let mut store = Store::new(engine, ()).expect("the engine makes a store");
    let module = Module::compile(engine, bytes)
        .await
        .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[])
        .await
        .expect("the module instantiates");
    (store, instance)
}

/// The export `name` of `instance`.
fn export(store: &mut Store<()>, instance: Instance, name: &str) -> Extern {
    instance
        .get_export(store, name)
        .expect("the instance belongs to the store")
        .unwrap_or_else(|| panic!("the instance exports `{name}`"))
}

/// The function `instance` exports as `name`.
fn func(store: &mut Store<()>, instance: Instance, name: &str) -> Func {
    export(store, instance, name)
        .into_func()
        .unwrap_or_else(|| panic!("`{name}` is a function"))
}

/// The `WebAssembly` namespace object of the page.
fn webassembly() -> JsValue {
    Reflect::get(&js_sys::global(), &"WebAssembly".into()).expect("the page has WebAssembly")
}

/// Runs `body` while the property `name` of the `WebAssembly` namespace
/// is `value`, and puts the property back after.
fn with_webassembly_property<R>(name: &str, value: &JsValue, body: impl FnOnce() -> R) -> R {
    let namespace = webassembly();
    let original = Reflect::get(&namespace, &name.into()).expect("the property reads");
    Reflect::set(&namespace, &name.into(), value).expect("the property is writable");
    let result = body();
    Reflect::set(&namespace, &name.into(), &original).expect("the property is writable");
    result
}

#[wcmp_macros::test]
fn it_declares_what_chromium_implements_and_not_host_suspension() {
    let capabilities = engine().capabilities();

    for capability in [
        Capability::MultiMemory,
        Capability::Memory64,
        Capability::TailCall,
        Capability::Exceptions,
        Capability::FunctionReferences,
        Capability::Gc,
        Capability::RelaxedSimd,
        Capability::Threads,
    ] {
        assert!(capabilities.contains(capability), "declares {capability}");
    }
    assert!(!capabilities.contains(Capability::HostSuspension));
    assert!(Web::new().has_jspi(), "Chromium has JSPI");
}

#[wcmp_macros::test]
async fn it_loads_and_declares_less_where_a_probe_fails() {
    let declared = engine().capabilities();
    assert!(declared.contains(Capability::Gc));

    // Every probe fails: the browser's `WebAssembly.validate` refuses each
    // probe module.
    let refuse = Closure::<dyn Fn(JsValue) -> bool>::new(|_| false);
    let refusing = with_webassembly_property("validate", refuse.as_ref(), engine);
    assert_eq!(refusing.capabilities(), Capabilities::empty());
    assert!(
        declared.iter().count() > refusing.capabilities().iter().count(),
        "the backend declares less: {declared:?} before"
    );

    // The backend loads, and runs a module of the floor.
    let (mut store, instance) = instance(
        &refusing,
        wasm!(r#"(module (func (export "answer") (result i32) i32.const 42))"#),
    )
    .await;
    let answer = func(&mut store, instance, "answer");
    let mut result = [Val::I32(0)];
    answer
        .call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(42));

    // A probe that throws leaves its capability out too.
    let throw = Closure::<dyn Fn(JsValue) -> Result<bool, JsValue>>::new(|_| {
        Err(JsValue::from_str("the probe throws"))
    });
    let throwing = with_webassembly_property("validate", throw.as_ref(), engine);
    assert_eq!(throwing.capabilities(), Capabilities::empty());
}

#[wcmp_macros::test]
async fn it_loads_without_javascript_promise_integration() {
    let backend = with_webassembly_property("Suspending", &JsValue::UNDEFINED, Web::new);
    assert!(!backend.has_jspi(), "the backend read no `Suspending`");
    let engine = Engine::with_backend(backend);
    assert!(!engine.capabilities().contains(Capability::HostSuspension));

    let (mut store, instance) = instance(
        &engine,
        wasm!(r#"(module (func (export "answer") (result i32) i32.const 42))"#),
    )
    .await;
    let mut result = [Val::I32(0)];
    func(&mut store, instance, "answer")
        .call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(42));
}

/// A module above the browser's limit for a synchronous compile: a memory
/// with a data segment of 9 MiB, and a function that reads the last byte
/// of the segment.
fn large_module() -> Vec<u8> {
    use wasm_encoder::{
        CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function, FunctionSection,
        MemArg, MemorySection, TypeSection, ValType,
    };

    const LEN: u32 = 9 * 1024 * 1024;
    let mut module = wasm_encoder::Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut memories = MemorySection::new();
    memories.memory(wasm_encoder::MemoryType {
        minimum: u64::from(LEN.div_ceil(65_536)),
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export("last", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instructions()
        .i32_const((LEN - 1) as i32)
        .i32_load8_u(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        })
        .end();
    code.function(&body);
    module.section(&code);
    let mut data = DataSection::new();
    data.active(
        0,
        &ConstExpr::i32_const(0),
        (0..LEN).map(|index| (index % 251) as u8),
    );
    module.section(&data);
    module.finish()
}

#[wcmp_macros::test]
async fn it_compiles_and_instantiates_a_module_above_the_synchronous_limit() {
    let engine = engine();
    let bytes = large_module();
    assert!(bytes.len() > SYNCHRONOUS_LIMIT, "{} bytes", bytes.len());

    let module = Module::compile(&engine, &bytes)
        .await
        .expect("the asynchronous compile loads the module");
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let instance = Instance::instantiate(&mut store, &module, &[])
        .await
        .expect("the asynchronous instantiation loads the module");
    let last = func(&mut store, instance, "last");
    let mut result = [Val::I32(0)];
    last.call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some((9 * 1024 * 1024 - 1) % 251));

    let synchronous = Module::new(&engine, &bytes);
    match synchronous {
        Err(Error::Compile { message }) => {
            assert!(!message.is_empty(), "the error carries the browser's words");
        }
        other => panic!("the synchronous compile fails with a compile error: {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_links_an_exported_function_as_its_own_function_object() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let exporter = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (func (export "double") (param i32) (result i32)
                local.get 0
                i32.const 2
                i32.mul))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let exporter = Instance::instantiate(&mut store, &exporter, &[])
        .await
        .expect("the module instantiates");
    let double = func(&mut store, exporter, "double");

    let importer = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "peer" "double" (func $double (param i32) (result i32)))
              (export "again" (func $double))
              (func (export "quadruple") (param i32) (result i32)
                local.get 0
                call $double
                call $double))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let importer = Instance::instantiate(&mut store, &importer, &[double.into()])
        .await
        .expect("the module instantiates");

    // The browser re-exports an imported function as the same function
    // object, so the re-export is the handle of the original export: the
    // import received the export's own function object.
    let again = func(&mut store, importer, "again");
    assert_eq!(again.index(), double.index());

    let quadruple = func(&mut store, importer, "quadruple");
    let mut result = [Val::I32(0)];
    quadruple
        .call(&mut store, &[Val::I32(5)], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(20));
}

#[wcmp_macros::test]
async fn it_refuses_an_import_of_the_wrong_kind_or_type_with_a_link_error() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let exporter = Module::compile(&engine, wasm!(r#"(module (func (export "nothing")))"#))
        .await
        .expect("the module compiles");
    let exporter = Instance::instantiate(&mut store, &exporter, &[])
        .await
        .expect("the module instantiates");
    let wrong_type = func(&mut store, exporter, "nothing");
    let wrong_kind = Global::new(
        &mut store,
        GlobalType::new(ValType::I32, Mutability::Const),
        Val::I32(0),
    )
    .expect("the store makes a global");

    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "first" (func))
              (import "host" "notify" (func (param i32))))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    for import in [wrong_type.into(), wrong_kind.into()] {
        let refused =
            Instance::instantiate(&mut store, &module, &[wrong_type.into(), import]).await;
        match refused {
            Err(Error::Link { module, name, .. }) => {
                assert_eq!((module.as_str(), name.as_str()), ("host", "notify"));
            }
            other => panic!("the instantiation fails with a link error: {other:?}"),
        }
    }
}

#[wcmp_macros::test]
async fn it_reads_and_writes_the_externs_the_browser_makes() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");

    let global = Global::new(
        &mut store,
        GlobalType::new(ValType::I64, Mutability::Var),
        Val::I64(-3),
    )
    .expect("the store makes a global");
    global
        .set(&mut store, Val::I64(1 << 40))
        .expect("the global is mutable");
    assert_eq!(
        global.get(&mut store).expect("the global reads").i64(),
        Some(1 << 40)
    );

    let memory = Memory::new(&mut store, MemoryType::new(1, Some(3))).expect("a memory");
    memory
        .write(&mut store, 65_530, b"edge")
        .expect("the range lies inside the memory");
    let mut read = [0; 4];
    memory
        .read(&store, 65_530, &mut read)
        .expect("the range lies inside the memory");
    assert_eq!(&read, b"edge");
    let outside = memory.read(&store, 65_534, &mut read);
    assert!(
        matches!(outside, Err(Error::MemoryOutOfBounds { .. })),
        "{outside:?}"
    );
    assert_eq!(memory.grow(&mut store, 2).expect("the memory grows"), 1);
    assert_eq!(
        memory.size(&store).expect("the memory has a size"),
        3 * 65_536
    );
    let beyond = memory.grow(&mut store, 1);
    assert!(matches!(beyond, Err(Error::Grow { .. })), "{beyond:?}");

    let table = Table::new(
        &mut store,
        TableType::new(RefType::FUNCREF, 2, None),
        Val::FuncRef(None),
    )
    .expect("the store makes a table");
    let instance = Instance::instantiate(
        &mut store,
        &Module::compile(
            &engine,
            wasm!(
                r#"
                (module
                  (import "host" "limit" (global $limit (mut i64)))
                  (import "host" "heap" (memory 1))
                  (import "host" "table" (table 2 funcref))
                  (func $seven (result i32) i32.const 7)
                  (elem (i32.const 1) func $seven)
                  (func (export "read") (result i64 i32)
                    global.get $limit
                    i32.const 65530
                    i32.load8_u))
                "#
            ),
        )
        .await
        .expect("the module compiles"),
        &[global.into(), memory.into(), table.into()],
    )
    .await
    .expect("the module instantiates");

    let read = func(&mut store, instance, "read");
    let mut results = [Val::I64(0), Val::I32(0)];
    read.call(&mut store, &[], &mut results)
        .expect("the call succeeds");
    assert_eq!(results[0].i64(), Some(1 << 40));
    assert_eq!(results[1].i32(), Some(i32::from(b'e')));

    let Val::FuncRef(Some(seven)) = table.get(&mut store, 1).expect("the index is inside") else {
        panic!("the guest set the element");
    };
    let mut result = [Val::I32(0)];
    seven
        .call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(7));
    let outside = table.get(&mut store, 2);
    assert!(
        matches!(outside, Err(Error::TableOutOfBounds { index: 2, size: 2 })),
        "{outside:?}"
    );
    assert_eq!(
        table
            .grow(&mut store, 3, Val::FuncRef(Some(seven)))
            .expect("the table grows"),
        2
    );
    assert_eq!(table.size(&store).expect("the table has a size"), 5);
}

#[wcmp_macros::test]
async fn it_carries_a_v128_through_a_generated_module() {
    let engine = engine();
    let (mut store, instance) = instance(
        &engine,
        wasm!(
            r#"
            (module
              (func $splat (export "splat") (param i32) (result v128)
                local.get 0
                i32x4.splat)
              (func (export "swap") (param v128 i64) (result i64 v128)
                local.get 1
                local.get 0
                local.get 0
                i8x16.shuffle 8 9 10 11 12 13 14 15 0 1 2 3 4 5 6 7)
              (elem declare func $splat)
              (func (export "unnamed") (result funcref)
                ref.func $splat))
            "#
        ),
    )
    .await;

    let splat = func(&mut store, instance, "splat");
    let mut result = [Val::V128(0)];
    splat
        .call(&mut store, &[Val::I32(-2)], &mut result)
        .expect("the carrier carries the v128 result");
    assert_eq!(
        result[0].v128(),
        Some(u128::from_le_bytes(
            [0xfe, 0xff, 0xff, 0xff]
                .repeat(4)
                .try_into()
                .expect("16 bytes")
        ))
    );

    let swap = func(&mut store, instance, "swap");
    let mut results = [Val::I64(0), Val::V128(0)];
    swap.call(
        &mut store,
        &[
            Val::V128(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210),
            Val::I64(9),
        ],
        &mut results,
    )
    .expect("the carrier carries the v128 parameter");
    assert_eq!(results[0].i64(), Some(9));
    assert_eq!(
        results[1].v128(),
        Some(0xfedc_ba98_7654_3210_0123_4567_89ab_cdef)
    );

    // The splat that a guest hands out is the export itself, so it keeps
    // the export's type and its carrier.
    let unnamed = func(&mut store, instance, "unnamed");
    let mut handed = [Val::FuncRef(None)];
    unnamed
        .call(&mut store, &[], &mut handed)
        .expect("the call succeeds");
    let Val::FuncRef(Some(handed)) = handed[0] else {
        panic!("the guest hands out a function: {handed:?}");
    };
    assert!(
        handed
            .ty(&store)
            .expect("the function belongs to the store")
            .is_some()
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_v128_for_a_function_whose_type_it_does_not_know() {
    let engine = engine();
    let (mut store, instance) = instance(
        &engine,
        wasm!(
            r#"
            (module
              (func $splat (param i32) (result v128)
                local.get 0
                i32x4.splat)
              (elem declare func $splat)
              (func (export "hand") (result funcref)
                ref.func $splat))
            "#
        ),
    )
    .await;
    let hand = func(&mut store, instance, "hand");
    let mut handed = [Val::FuncRef(None)];
    hand.call(&mut store, &[], &mut handed)
        .expect("the call succeeds");
    let Val::FuncRef(Some(splat)) = handed[0] else {
        panic!("the guest hands out a function: {handed:?}");
    };
    assert_eq!(
        splat.ty(&store).expect("the function belongs to the store"),
        None
    );
    let mut result = [Val::V128(0)];
    let refused = splat.call(&mut store, &[Val::I32(1)], &mut result);
    assert!(
        matches!(refused, Err(Error::TypeMismatch { .. })),
        "{refused:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_host_function_until_it_has_a_wrapper() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let refused = Func::new(
        &mut store,
        wcmp_wasm_core::FuncType::new([], []),
        |_, _, _| Ok(()),
    );
    assert!(matches!(refused, Err(Error::Backend { .. })), "{refused:?}");
}
