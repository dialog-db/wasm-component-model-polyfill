//! What the browser backend holds beyond the backend contract: the probes
//! of its capabilities, a module above the browser's limit for a
//! synchronous compile, linking by the objects of the browser, the externs
//! the browser makes, host functions through their wrapper modules, how
//! memory access crosses into JavaScript, and what host suspension does
//! inside a host function.

#![cfg(target_arch = "wasm32")]

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Array, Function, Object, Proxy, Reflect, SharedArrayBuffer, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wcmp_macros::wasm;
use wcmp_wasm_core::backend::RawHandle;
use wcmp_wasm_core::{
    Caller, Capabilities, Capability, Engine, Error, Extern, ExternRef, Func, FuncType, Global,
    GlobalType, Instance, Memory, MemoryType, Module, Mutability, RefType, ResumableCall, Store,
    SuspendedCall, Table, TableType, TrapKind, Val, ValType,
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
fn it_declares_what_chromium_implements() {
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
        Capability::HostSuspension,
    ] {
        assert!(capabilities.contains(capability), "declares {capability}");
    }
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
    // Host suspension rests on JavaScript Promise Integration, which no
    // probe module tests.
    let unprobed = Capabilities::empty().with(Capability::HostSuspension);
    assert_eq!(refusing.capabilities(), unprobed);
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
    assert_eq!(throwing.capabilities(), unprobed);
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

#[wcmp_macros::test]
fn it_declares_host_suspension_only_where_both_functions_of_jspi_exist() {
    for name in ["Suspending", "promising"] {
        let backend = with_webassembly_property(name, &JsValue::UNDEFINED, Web::new);
        assert!(!backend.has_jspi(), "the backend read no `{name}`");
        let engine = Engine::with_backend(backend);
        assert!(
            !engine.capabilities().contains(Capability::HostSuspension),
            "without `{name}`"
        );
    }
    let backend = Web::new();
    assert!(backend.has_jspi());
    assert!(
        Engine::with_backend(backend)
            .capabilities()
            .contains(Capability::HostSuspension)
    );
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

/// The bytes 1 to 16, a pattern no fresh memory holds.
const PATTERN: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

/// The property `name` of `target`.
fn property(target: &JsValue, name: &str) -> JsValue {
    Reflect::get(target, &name.into()).expect("the property reads")
}

/// A proxy of `target` whose trap `trap` is `closure`.
fn proxy(target: &JsValue, trap: &str, closure: &JsValue) -> JsValue {
    let handler = Object::new();
    Reflect::set(&handler, &trap.into(), closure).expect("the handler is an object");
    Proxy::new(target, &handler).into()
}

/// Runs `body`, and counts the calls of `TypedArray.prototype.set` on a
/// `Uint8Array` meanwhile: the one step of JavaScript a bulk copy takes
/// where the browser lacks `multi_memory`.
fn typed_array_sets<R>(body: impl FnOnce() -> R) -> (R, u32) {
    let prototype = property(&property(&js_sys::global(), "Uint8Array"), "prototype");
    let set = property(&prototype, "set");
    let count = Rc::new(Cell::new(0));
    let apply = Closure::<dyn Fn(JsValue, JsValue, JsValue) -> Result<JsValue, JsValue>>::new({
        let count = count.clone();
        move |target: JsValue, this: JsValue, args: JsValue| {
            count.set(count.get() + 1);
            Reflect::apply(
                target.unchecked_ref::<Function>(),
                &this,
                args.unchecked_ref(),
            )
        }
    });
    Reflect::set(
        &prototype,
        &"set".into(),
        &proxy(&set, "apply", apply.as_ref()),
    )
    .expect("the prototype is writable");
    let result = body();
    Reflect::delete_property(prototype.unchecked_ref::<Object>(), &"set".into())
        .expect("the prototype is writable");
    (result, count.get())
}

/// An engine over a backend made while the browser refuses the probe of
/// `multi_memory`, as a browser without the feature does, and accepts
/// every other.
fn engine_without_multi_memory() -> Engine {
    let validate = property(&webassembly(), "validate").unchecked_into::<Function>();
    let probe = wasm!(r#"(module (memory 1) (memory 1))"#);
    let refuse = Closure::<dyn Fn(JsValue) -> Result<JsValue, JsValue>>::new(move |bytes| {
        if Uint8Array::new(&bytes).to_vec() == probe {
            return Ok(JsValue::FALSE);
        }
        Reflect::apply(&validate, &webassembly(), &Array::of1(&bytes))
    });
    with_webassembly_property("validate", refuse.as_ref(), engine)
}

/// The bytes of `memory` at `offset`, read with no count of JavaScript.
fn bytes_at(store: &Store<()>, memory: Memory, offset: u64) -> Vec<u8> {
    memory
        .with_bytes(store, offset, 16, <[u8]>::to_vec)
        .expect("the range lies inside the memory")
}

#[wcmp_macros::test]
async fn it_copies_in_bulk_with_no_javascript_where_multi_memory_is_declared() {
    let engine = engine();
    assert!(engine.capabilities().contains(Capability::MultiMemory));
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let first = Memory::new(&mut store, MemoryType::new(1, None)).expect("a memory");
    let second = Memory::new(&mut store, MemoryType::new(1, None)).expect("a memory");
    let shared = Memory::new(&mut store, MemoryType::shared(1, 1)).expect("a shared memory");
    // The first access of a memory, and the first copy between two, make
    // their generated modules.
    for memory in [first, second, shared] {
        memory.size(&store).expect("the memory is the store's");
    }
    Memory::copy(&mut store, &first, 0, &second, 0, 1).expect("the ranges lie inside");
    Memory::copy(&mut store, &first, 0, &shared, 0, 1).expect("the ranges lie inside");
    Memory::copy(&mut store, &shared, 0, &second, 0, 1).expect("the ranges lie inside");

    let ((), sets) = typed_array_sets(|| {
        for memory in [first, shared] {
            memory
                .write(&mut store, 8, &PATTERN)
                .expect("the range lies inside the memory");
            let mut read = [0; 16];
            memory
                .read(&store, 8, &mut read)
                .expect("the range lies inside the memory");
            assert_eq!(read, PATTERN);
            assert_eq!(bytes_at(&store, memory, 8), PATTERN);
            assert_eq!(memory.load_u32(&store, 8).ok(), Some(0x0403_0201));
        }
        Memory::copy(&mut store, &first, 8, &second, 100, 16).expect("the ranges lie inside");
        Memory::copy(&mut store, &shared, 8, &second, 200, 16).expect("the ranges lie inside");
        Memory::copy(&mut store, &first, 8, &first, 12, 16).expect("the ranges lie inside");
    });
    assert_eq!(
        sets, 0,
        "a bulk copy is a `memory.copy`, or an atomic copy of each byte, in WebAssembly"
    );
    assert_eq!(bytes_at(&store, second, 100), PATTERN);
    assert_eq!(bytes_at(&store, second, 200), PATTERN);
    assert_eq!(bytes_at(&store, first, 12), PATTERN);
}

#[wcmp_macros::test]
async fn it_copies_in_bulk_with_one_typed_array_set_where_multi_memory_is_off() {
    let engine = engine_without_multi_memory();
    let capabilities = engine.capabilities();
    assert!(!capabilities.contains(Capability::MultiMemory));
    assert!(
        capabilities.contains(Capability::Threads),
        "only the probe of `multi_memory` fails"
    );
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let first = Memory::new(&mut store, MemoryType::new(1, None)).expect("a memory");
    let second = Memory::new(&mut store, MemoryType::new(1, None)).expect("a memory");
    let shared = Memory::new(&mut store, MemoryType::shared(1, 1)).expect("a shared memory");
    for memory in [first, second, shared] {
        memory.size(&store).expect("the memory is the store's");
    }

    let (_, write) = typed_array_sets(|| first.write(&mut store, 8, &PATTERN));
    let mut read = [0; 16];
    let (_, reads) = typed_array_sets(|| first.read(&store, 8, &mut read));
    assert_eq!(read, PATTERN);
    let (lent, lends) = typed_array_sets(|| first.with_bytes(&store, 8, 16, <[u8]>::to_vec));
    assert_eq!(lent.ok().as_deref(), Some(&PATTERN[..]));
    let (_, copies) = typed_array_sets(|| Memory::copy(&mut store, &first, 8, &second, 100, 16));
    assert_eq!(bytes_at(&store, second, 100), PATTERN);
    assert_eq!(
        [write, reads, lends, copies],
        [1; 4],
        "each bulk copy (`write`, `read`, `with_bytes`, `copy`) is one `TypedArray.set`"
    );

    // A scalar access, and a copy within one memory, stay in WebAssembly.
    let (loaded, scalars) = typed_array_sets(|| {
        first.store_u16(&mut store, 40, 0xbeef).expect("inside");
        first.load_u32(&store, 8)
    });
    assert_eq!(loaded.ok(), Some(0x0403_0201));
    let (_, within) = typed_array_sets(|| Memory::copy(&mut store, &first, 8, &first, 12, 16));
    assert_eq!(bytes_at(&store, first, 12), PATTERN);
    assert_eq!([scalars, within], [0, 0]);

    // A shared memory takes each byte atomically, through its accessor,
    // and never through `TypedArray.set`.
    let ((), atomic) = typed_array_sets(|| {
        shared.write(&mut store, 8, &PATTERN).expect("inside");
        assert_eq!(bytes_at(&store, shared, 8), PATTERN);
        Memory::copy(&mut store, &shared, 8, &second, 300, 16).expect("inside");
        Memory::copy(&mut store, &second, 300, &shared, 100, 16).expect("inside");
    });
    assert_eq!(atomic, 0);
    assert_eq!(bytes_at(&store, second, 300), PATTERN);
    assert_eq!(bytes_at(&store, shared, 100), PATTERN);
}

#[wcmp_macros::test]
async fn it_lends_a_copy_of_a_shared_memory() {
    // The runner of the web lane serves every page cross-origin isolated,
    // so this test cannot show a page without isolation. The backend asks
    // for none: `new WebAssembly.Memory({ shared: true })` needs none.
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");

    // The test keeps the browser's object of the memory the backend makes,
    // so that it can write the memory as another agent would.
    let made = Rc::new(RefCell::new(None));
    let construct =
        Closure::<dyn Fn(JsValue, JsValue, JsValue) -> Result<JsValue, JsValue>>::new({
            let made = made.clone();
            move |target: JsValue, args: JsValue, new_target: JsValue| {
                let memory = Reflect::construct_with_new_target(
                    target.unchecked_ref(),
                    args.unchecked_ref(),
                    new_target.unchecked_ref(),
                )?;
                made.replace(Some(memory.clone()));
                Ok(memory)
            }
        });
    let constructor = property(&webassembly(), "Memory");
    let memory = with_webassembly_property(
        "Memory",
        &proxy(&constructor, "construct", construct.as_ref()),
        || Memory::new(&mut store, MemoryType::shared(1, 1)),
    )
    .expect("the store makes a shared memory");
    let buffer = property(
        made.borrow().as_ref().expect("the backend made the memory"),
        "buffer",
    );
    assert!(buffer.is_instance_of::<SharedArrayBuffer>());

    memory
        .write(&mut store, 100, &PATTERN)
        .expect("the range lies inside the memory");
    let other_agent = Uint8Array::new_with_byte_offset_and_length(&buffer, 100, 16);
    let lent = memory
        .with_bytes(&store, 100, 16, |bytes| {
            // Another agent writes the range while the bytes are lent.
            other_agent.fill(0xee, 0, 16);
            bytes.to_vec()
        })
        .expect("the range lies inside the memory");
    assert_eq!(lent, PATTERN, "the lent bytes are a copy");
    assert_eq!(
        memory.load_u8(&store, 100).ok(),
        Some(0xee),
        "the memory holds the write of the other agent"
    );
}

#[wcmp_macros::test]
async fn it_reads_and_writes_guest_memory_from_a_host_function() {
    let engine = engine();
    assert!(engine.capabilities().contains(Capability::MultiMemory));
    let mut store = Store::new(&engine, None::<Memory>).expect("the engine makes a store");
    // The host function reads the `u32` the guest stored at the address it
    // passes, writes its bytes back reversed, and stores the value plus one
    // after them.
    let reverse = Func::new(
        &mut store,
        FuncType::new([ValType::I32], []),
        |mut caller: Caller<'_, Option<Memory>>, params, _| {
            let memory = caller
                .data()
                .ok_or_else(|| anyhow::anyhow!("the guest memory is not set"))?;
            let address = u64::from(params[0].i32().unwrap_or_default() as u32);
            let value = memory.load_u32(&caller, address)?;
            let mut bytes = [0; 4];
            memory.read(&caller, address, &mut bytes)?;
            anyhow::ensure!(bytes == value.to_le_bytes(), "the read and the load differ");
            bytes.reverse();
            memory.write(&mut caller, address, &bytes)?;
            memory.store_u32(&mut caller, address + 4, value + 1)?;
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "reverse" (func $reverse (param i32)))
              (memory (export "memory") 1)
              (func (export "run") (param i32) (result i32 i32)
                local.get 0
                i32.const 0x04030201
                i32.store
                local.get 0
                call $reverse
                local.get 0
                i32.load
                local.get 0
                i32.load offset=4))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[reverse.into()])
        .await
        .expect("the module instantiates");
    let memory = instance
        .get_export(&mut store, "memory")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_memory)
        .expect("the instance exports `memory`");
    let run = instance
        .get_export(&mut store, "run")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports `run`");
    *store.data_mut() = Some(memory);

    // The first call makes the accessor of the memory inside the host
    // function. The second finds it made, and crosses into no
    // `TypedArray.set`.
    let call = |store: &mut Store<Option<Memory>>, address: i32| {
        let mut results = [Val::I32(0), Val::I32(0)];
        run.call(store, &[Val::I32(address)], &mut results)
            .expect("the call succeeds");
        (results[0].i32(), results[1].i32())
    };
    assert_eq!(call(&mut store, 64), (Some(0x0102_0304), Some(0x0403_0202)));
    let (seen, sets) = typed_array_sets(|| call(&mut store, 128));
    assert_eq!(
        seen,
        (Some(0x0102_0304), Some(0x0403_0202)),
        "the guest sees each write of the host function"
    );
    assert_eq!(
        sets, 0,
        "the host function reaches memory through its accessor"
    );
    assert_eq!(memory.load_u32(&store, 132).ok(), Some(0x0403_0202));
}

/// Polls `future` once, as a host function, which cannot wait, does.
fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
}

/// What the store of the test of suspension inside a host function holds:
/// the guest function that waits, the call a host function started and
/// that waits, and what the host function's resumption gave.
#[derive(Default)]
struct Inside {
    run: Option<Func>,
    waiting: Option<SuspendedCall>,
    resumed: Option<Result<(), Error>>,
}

#[wcmp_macros::test]
async fn it_starts_a_resumable_call_inside_a_host_function_and_resumes_it_outside() {
    let engine = engine();
    let mut store = Store::new(&engine, Inside::default()).expect("the engine makes a store");
    let wait = Func::new_suspending(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |_, _, _| Ok(Poll::Pending),
    )
    .expect("the store makes a suspending host function");
    // A resumable call from a host function runs on a stack of its own,
    // so only WebAssembly frames lie between its start and `wait`, and it
    // suspends on the future's first poll.
    let start = Func::new(
        &mut store,
        FuncType::new([], []),
        |mut caller: Caller<'_, Inside>, _, _| {
            let run = caller
                .data()
                .run
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            let mut results = [Val::I32(0)];
            let Poll::Ready(outcome) =
                poll_once(run.call_resumable(&mut caller, &[Val::I32(3)], &mut results))
            else {
                anyhow::bail!("the call did not suspend at once");
            };
            match outcome? {
                ResumableCall::Suspended(handle) => caller.data_mut().waiting = Some(handle),
                other => anyhow::bail!("the call did not suspend: {other:?}"),
            }
            Ok(())
        },
    )
    .expect("the store makes a host function");
    // A host function cannot resume a call: the browser would run it only
    // after the host function returned.
    let resume = Func::new(
        &mut store,
        FuncType::new([], []),
        |mut caller: Caller<'_, Inside>, _, _| {
            let handle = caller
                .data_mut()
                .waiting
                .take()
                .ok_or_else(|| anyhow::anyhow!("no call waits"))?;
            let mut results = [Val::I32(0)];
            let resumed = match poll_once(handle.resume(&mut caller, &[Val::I32(4)], &mut results))
            {
                Poll::Ready(outcome) => outcome.map(|_| ()),
                Poll::Pending => Ok(()),
            };
            caller.data_mut().resumed = Some(resumed);
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "wait" (func $wait (param i32) (result i32)))
              (import "host" "start" (func $start))
              (import "host" "resume" (func $resume))
              (func (export "run") (param i32) (result i32)
                local.get 0
                call $wait
                local.get 0
                i32.add)
              (func (export "start") call $start)
              (func (export "resume") call $resume))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(
        &mut store,
        &module,
        &[wait.into(), start.into(), resume.into()],
    )
    .await
    .expect("the module instantiates");
    let export = |store: &mut Store<Inside>, name| {
        instance
            .get_export(store, name)
            .expect("the instance belongs to the store")
            .and_then(Extern::into_func)
            .expect("the instance exports the function")
    };
    let run = export(&mut store, "run");
    store.data_mut().run = Some(run);

    export(&mut store, "start")
        .call(&mut store, &[], &mut [])
        .expect("the host function starts the call");
    let handle = store.data_mut().waiting.take().expect("the call waits");
    let mut results = [Val::I32(0)];
    let outcome = handle
        .resume(&mut store, &[Val::I32(10)], &mut results)
        .await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert_eq!(results[0].i32(), Some(13));

    export(&mut store, "start")
        .call(&mut store, &[], &mut [])
        .expect("the host function starts the call");
    export(&mut store, "resume")
        .call(&mut store, &[], &mut [])
        .expect("the host function returns");
    match store.data_mut().resumed.take() {
        Some(Err(Error::Backend { message })) => {
            assert!(message.contains("cannot resume"), "{message}");
        }
        other => panic!("the resumption fails with a backend error: {other:?}"),
    }
}

#[wcmp_macros::test]
async fn it_traps_a_suspension_outside_every_resumable_call() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let wait = Func::new_suspending(&mut store, FuncType::new([], []), |_, _, _| {
        Ok(Poll::Pending)
    })
    .expect("the store makes a suspending host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "wait" (func $wait))
              (func (export "run") call $wait))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[wait.into()])
        .await
        .expect("the module instantiates");
    let run = instance
        .get_export(&mut store, "run")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports `run`");
    match run.call(&mut store, &[], &mut []) {
        Err(Error::Trap(TrapKind::Host(error))) => {
            assert!(error.to_string().contains("cannot suspend"), "{error}");
        }
        other => panic!("the call traps with the host's error: {other:?}"),
    }
    // Inside a resumable call, the same host function suspends the call.
    let mut results = [];
    let outcome = run.call_resumable(&mut store, &[], &mut results).await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Suspended(_))),
        "{outcome:?}"
    );
}
