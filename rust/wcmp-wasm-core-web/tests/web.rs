//! What the browser backend holds beyond the backend contract: the probes
//! of its capabilities, a module above the browser's limit for a
//! synchronous compile, linking by the objects of the browser, the externs
//! the browser makes, and host functions through their wrapper modules.

#![cfg(target_arch = "wasm32")]

use js_sys::Reflect;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wcmp_macros::wasm;
use wcmp_wasm_core::backend::RawHandle;
use wcmp_wasm_core::{
    Caller, Capabilities, Capability, Engine, Error, Extern, ExternRef, Func, FuncType, Global,
    GlobalType, Instance, Memory, MemoryType, Module, Mutability, RefType, Store, Table, TableType,
    TrapKind, Val, ValType,
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

/// The host function of `ty` in `store` that gives back its arguments as
/// its results.
fn echo<T: 'static>(store: &mut Store<T>, ty: FuncType) -> Func {
    Func::new(store, ty, |_, params, results| {
        results.copy_from_slice(params);
        Ok(())
    })
    .expect("the store makes a host function")
}

#[wcmp_macros::test]
async fn it_carries_the_bits_of_each_value_through_a_host_function() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let types = [
        ValType::F32,
        ValType::F64,
        ValType::V128,
        ValType::EXTERNREF,
        ValType::FUNCREF,
    ];
    let same = echo(&mut store, FuncType::new(types.clone(), types));
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "same"
                (func $same
                  (param f32 f64 v128 externref funcref)
                  (result f32 f64 v128 externref funcref)))
              (func $marker (result i32) i32.const 42)
              (elem declare func $marker)
              ;; The guest makes each value itself, so only the wrapper
              ;; carries it, and gives back the bits of each number.
              (func (export "relay") (param $token externref)
                (result i32 i64 i64 i64 externref funcref)
                (local $single f32)
                (local $double f64)
                (local $vector v128)
                (local $held externref)
                (local $function funcref)
                i32.const 0x7fa00001
                f32.reinterpret_i32
                i64.const 0x7ff4000000000001
                f64.reinterpret_i64
                v128.const i64x2 0x0123456789abcdef 0x7edcba9876543210
                local.get $token
                ref.func $marker
                call $same
                local.set $function
                local.set $held
                local.set $vector
                local.set $double
                local.set $single
                local.get $single
                i32.reinterpret_f32
                local.get $double
                i64.reinterpret_f64
                local.get $vector
                i64x2.extract_lane 0
                local.get $vector
                i64x2.extract_lane 1
                local.get $held
                local.get $function))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[same.into()])
        .await
        .expect("the module instantiates");
    let relay = func(&mut store, instance, "relay");
    let token = ExternRef::new(&mut store, "token").expect("the store makes an externref");

    let mut results = [
        Val::I32(0),
        Val::I64(0),
        Val::I64(0),
        Val::I64(0),
        Val::ExternRef(None),
        Val::FuncRef(None),
    ];
    relay
        .call(&mut store, &[Val::ExternRef(Some(token))], &mut results)
        .expect("the call succeeds");

    // A signaling NaN keeps its payload each way, which a `Number` would
    // not promise.
    assert_eq!(results[0].i32(), Some(0x7fa0_0001));
    assert_eq!(results[1].i64(), Some(0x7ff4_0000_0000_0001));
    assert_eq!(results[2].i64(), Some(0x0123_4567_89ab_cdef));
    assert_eq!(results[3].i64(), Some(0x7edc_ba98_7654_3210));
    let Val::ExternRef(Some(held)) = results[4] else {
        panic!("the host gave back the externref: {:?}", results[4]);
    };
    assert_eq!(
        held.data(&store)
            .expect("the externref belongs to the store")
            .downcast_ref::<&str>(),
        Some(&"token")
    );
    let Val::FuncRef(Some(marker)) = results[5] else {
        panic!("the host gave back the funcref: {:?}", results[5]);
    };
    let mut result = [Val::I32(0)];
    marker
        .call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(42));
}

#[wcmp_macros::test]
fn it_carries_a_v128_parameter_to_a_host_function_through_a_carrier() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let same = echo(
        &mut store,
        FuncType::new([ValType::V128, ValType::I32], [ValType::V128, ValType::I32]),
    );
    let bits = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeff;
    let mut results = [Val::V128(0), Val::I32(0)];
    same.call(&mut store, &[Val::V128(bits), Val::I32(-5)], &mut results)
        .expect("the host calls its own function through a carrier");
    assert_eq!(results[0].v128(), Some(bits));
    assert_eq!(results[1].i32(), Some(-5));
}

#[wcmp_macros::test]
async fn it_carries_an_exnref_through_a_host_function() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let same = echo(
        &mut store,
        FuncType::new([ValType::EXNREF], [ValType::EXNREF]),
    );
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "same" (func $same (param exnref) (result exnref)))
              (tag $oops (param i32))
              (func (export "round") (param i32) (result i32)
                (local $held exnref)
                block $caught (result exnref)
                  try_table (catch_all_ref $caught)
                    local.get 0
                    throw $oops
                  end
                  unreachable
                end
                call $same
                local.set $held
                block $payload (result i32)
                  try_table (catch $oops $payload)
                    local.get $held
                    throw_ref
                  end
                  unreachable
                end))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[same.into()])
        .await
        .expect("the module instantiates");
    let round = func(&mut store, instance, "round");
    let mut result = [Val::I32(0)];
    round
        .call(&mut store, &[Val::I32(17)], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(17));
}

#[wcmp_macros::test]
async fn it_traps_where_a_host_function_gives_a_result_of_the_wrong_type() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let wrong = Func::new(
        &mut store,
        FuncType::new([], [ValType::I32]),
        |_, _, results| {
            results[0] = Val::I64(1);
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "wrong" (func $wrong (result i32)))
              (global $handled (export "handled") (mut i32) (i32.const 0))
              (func (export "guarded")
                block $caught
                  try_table (catch_all $caught)
                    call $wrong
                    drop
                  end
                  return
                end
                i32.const 1
                global.set $handled))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[wrong.into()])
        .await
        .expect("the module instantiates");
    let guarded = func(&mut store, instance, "guarded");

    match guarded.call(&mut store, &[], &mut []) {
        Err(Error::Trap(TrapKind::Host(error))) => assert!(
            matches!(
                error.downcast_ref::<Error>(),
                Some(Error::TypeMismatch { .. })
            ),
            "{error:?}"
        ),
        other => panic!("the call fails with the host's type mismatch: {other:?}"),
    }
    let handled = export(&mut store, instance, "handled")
        .into_global()
        .expect("`handled` is a global")
        .get(&mut store)
        .expect("the global belongs to the store");
    assert_eq!(handled.i32(), Some(0), "the guest's handler did not run");
}

#[wcmp_macros::test]
async fn it_calls_a_host_function_from_the_start_function() {
    let engine = engine();
    let mut store = Store::new(&engine, 0u32).expect("the engine makes a store");
    let count = Func::new(
        &mut store,
        FuncType::new([], []),
        |mut caller: Caller<'_, u32>, _, _| {
            *caller.data_mut() += 1;
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let fail = Func::new(&mut store, FuncType::new([], []), |_, _, _| {
        anyhow::bail!("the start refused")
    })
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "start" (func $start))
              (start $start))
            "#
        ),
    )
    .await
    .expect("the module compiles");

    Instance::instantiate(&mut store, &module, &[count.into()])
        .await
        .expect("the module instantiates");
    assert_eq!(*store.data(), 1, "the start function called the host");

    match Instance::instantiate(&mut store, &module, &[fail.into()]).await {
        Err(Error::Trap(TrapKind::Host(error))) => {
            assert_eq!(error.to_string(), "the start refused");
        }
        other => panic!("the instantiation fails with the host's error: {other:?}"),
    }
}

/// What the store of the test of a trap under a host function holds: the
/// guest function the host function calls back into.
#[derive(Default)]
struct Below {
    down: Option<Func>,
}

#[wcmp_macros::test]
async fn it_keeps_each_frame_of_a_host_function_below_a_trap() {
    let engine = engine();
    let mut store = Store::new(&engine, Below::default()).expect("the engine makes a store");
    // At depth zero the host function fails. At depth one it calls down,
    // and makes the failure below it a result. Above that it adds its
    // depth to what the call below gave.
    let descend = Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |mut caller: Caller<'_, Below>, params, results| {
            let depth = params[0].i32().unwrap_or_default();
            if depth == 0 {
                anyhow::bail!("the bottom refuses");
            }
            let down = caller
                .data()
                .down
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            let mut inner = [Val::I32(0)];
            let below = match down.call(&mut caller, &[Val::I32(depth - 1)], &mut inner) {
                Ok(()) => inner[0].i32().unwrap_or_default(),
                Err(Error::Trap(TrapKind::Host(error))) if depth == 1 => {
                    anyhow::ensure!(error.to_string() == "the bottom refuses");
                    100
                }
                Err(error) => return Err(error.into()),
            };
            anyhow::ensure!(params[0].i32() == Some(depth), "the arguments changed");
            results[0] = Val::I32(below + depth);
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "descend" (func $descend (param i32) (result i32)))
              (func (export "down") (param i32) (result i32)
                local.get 0
                call $descend))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[descend.into()])
        .await
        .expect("the module instantiates");
    let down = instance
        .get_export(&mut store, "down")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports `down`");
    store.data_mut().down = Some(down);

    // Twice, so the second call finds the frames as the first left them.
    for _ in 0..2 {
        let mut result = [Val::I32(0)];
        down.call(&mut store, &[Val::I32(4)], &mut result)
            .expect("the call succeeds");
        assert_eq!(result[0].i32(), Some(100 + 1 + 2 + 3 + 4));
    }
}
