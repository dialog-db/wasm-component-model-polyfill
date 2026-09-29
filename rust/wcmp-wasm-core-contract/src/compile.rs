//! Compilation, the boundary of a module, and instantiation.

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Engine, Error, ExportType, ExternType, Func, FuncType, Global, GlobalType, ImportType,
    Instance, MemoryType, Module, Mutability, RefType, TableType, Val, ValType,
};

use crate::support;

/// A module that adds two numbers.
const ADD: &[u8] = wasm!(
    r#"
    (module
      (func (export "add") (param i32 i32) (result i32)
        local.get 0
        local.get 1
        i32.add))
    "#
);

/// The asynchronous compile and the synchronous compile each give a
/// module of their own, and each module runs.
pub async fn it_compiles_a_module_asynchronously_and_synchronously(engine: &Engine) {
    let asynchronous = Module::compile(engine, ADD)
        .await
        .expect("the asynchronous compile succeeds");
    let synchronous = Module::new(engine, ADD).expect("the synchronous compile succeeds");

    for module in [asynchronous, synchronous] {
        let mut store = support::store(engine, ());
        let instance = Instance::instantiate(&mut store, &module, &[])
            .await
            .expect("the module instantiates");
        let add = support::func(&mut store, instance, "add");
        let sum = support::call(
            &mut store,
            add,
            &[Val::I32(2), Val::I32(3)],
            &[ValType::I32],
        );
        assert_eq!(sum[0].i32(), Some(5));
    }
}

/// Bytes that are not a module fail both compiles with
/// [`Error::Compile`], and neither panics.
pub async fn it_refuses_bytes_that_are_not_a_module_with_a_compile_error(engine: &Engine) {
    let bytes = b"\0asm\x01\0\0\0\x7f";

    let asynchronous = Module::compile(engine, bytes).await;
    assert!(
        matches!(asynchronous, Err(Error::Compile { .. })),
        "the asynchronous compile refuses the bytes: {asynchronous:?}"
    );
    let synchronous = Module::new(engine, bytes);
    assert!(
        matches!(synchronous, Err(Error::Compile { .. })),
        "the synchronous compile refuses the bytes: {synchronous:?}"
    );
}

/// The module describes each import and each export, in order, with its
/// type: a function, a global, a table, and a memory each way.
pub async fn it_describes_the_imports_and_exports_of_a_module(engine: &Engine) {
    let module = support::module(
        engine,
        wasm!(
            r#"
            (module
              (import "host" "log" (func (param i32) (result i64)))
              (import "host" "limit" (global (mut i32)))
              (import "host" "table" (table 1 8 funcref))
              (import "host" "memory" (memory 1 2))
              (func (export "run") (param f32 f64) (result v128)
                v128.const i64x2 0 0)
              (global (export "count") i64 (i64.const 7))
              (table (export "refs") 2 externref)
              (memory (export "heap") 1))
            "#
        ),
    )
    .await;

    assert_eq!(
        module.imports().cloned().collect::<Vec<_>>(),
        [
            ImportType::new(
                "host",
                "log",
                ExternType::Func(FuncType::new([ValType::I32], [ValType::I64])),
            ),
            ImportType::new(
                "host",
                "limit",
                ExternType::Global(GlobalType::new(ValType::I32, Mutability::Var)),
            ),
            ImportType::new(
                "host",
                "table",
                ExternType::Table(TableType::new(RefType::FUNCREF, 1, Some(8))),
            ),
            ImportType::new(
                "host",
                "memory",
                ExternType::Memory(MemoryType::new(1, Some(2))),
            ),
        ]
    );
    assert_eq!(
        module.exports().cloned().collect::<Vec<_>>(),
        [
            ExportType::new(
                "run",
                ExternType::Func(FuncType::new([ValType::F32, ValType::F64], [ValType::V128])),
            ),
            ExportType::new(
                "count",
                ExternType::Global(GlobalType::new(ValType::I64, Mutability::Const)),
            ),
            ExportType::new(
                "refs",
                ExternType::Table(TableType::new(RefType::EXTERNREF, 2, None)),
            ),
            ExportType::new("heap", ExternType::Memory(MemoryType::new(1, None))),
        ]
    );
}

/// An import given an extern of the wrong type, or of the wrong kind, fails
/// the instantiation with [`Error::Link`], which names the import.
pub async fn it_refuses_an_import_of_the_wrong_type_with_a_link_error(engine: &Engine) {
    let module = support::module(
        engine,
        wasm!(r#"(module (import "host" "notify" (func (param i32))))"#),
    )
    .await;
    let mut store = support::store(engine, ());
    let wrong_type = Func::new(&mut store, FuncType::new([], []), |_, _, _| Ok(()))
        .expect("the store makes a host function");
    let wrong_kind = Global::new(
        &mut store,
        GlobalType::new(ValType::I32, Mutability::Const),
        Val::I32(0),
    )
    .expect("the store makes a global");

    for import in [wrong_type.into(), wrong_kind.into()] {
        let refused = Instance::instantiate(&mut store, &module, &[import]).await;
        match refused {
            Err(Error::Link { module, name, .. }) => {
                assert_eq!((module.as_str(), name.as_str()), ("host", "notify"));
            }
            other => panic!("the instantiation fails with a link error: {other:?}"),
        }
    }
}
