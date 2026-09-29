//! What the Wasmtime backend holds beyond the backend contract: the
//! capabilities it declares, compiles and instantiations that finish at
//! once, and continuation types at the boundary.

#![cfg(not(target_arch = "wasm32"))]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Capability, Engine, Error, ExportType, Extern, ExternType, FuncType, HeapType, Instance,
    Module, RefType, Store, Val, ValType,
};
use wcmp_wasm_core_wasmtime::Wasmtime;

fn engine() -> Engine {
    Engine::with_backend(Wasmtime::new().expect("Wasmtime makes an engine"))
}

/// Whether Wasmtime's compiler serves stack switching on this platform.
const STACK_SWITCHING: bool = cfg!(all(
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "macos")
));

/// The output of `future`, which must be ready on its first poll.
fn at_once<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future is ready on its first poll"),
    }
}

#[wcmp_macros::test]
fn it_declares_what_wasmtime_implements_and_not_host_suspension() {
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
    assert_eq!(
        capabilities.contains(Capability::StackSwitching),
        STACK_SWITCHING
    );
    assert!(!capabilities.contains(Capability::HostSuspension));
}

#[wcmp_macros::test]
fn it_finishes_a_compile_and_an_instantiation_at_once() {
    let engine = engine();
    let bytes = wasm!(r#"(module (func (export "answer") (result i32) i32.const 42))"#);

    let module = at_once(Module::compile(&engine, bytes)).expect("the module compiles");
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let instance =
        at_once(Instance::instantiate(&mut store, &module, &[])).expect("the module instantiates");
    let answer = instance
        .get_export(&mut store, "answer")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports its function");
    let mut result = [Val::I32(0)];
    answer
        .call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(42));
}

#[wcmp_macros::test]
async fn it_lets_continuation_types_through_the_boundary() {
    if !STACK_SWITCHING {
        return;
    }
    let engine = engine();
    let bytes = wasm!(
        r#"
        (module
          (type $task (func))
          (type $continuation (cont $task))
          (tag (export "yield"))
          (func (export "take") (param (ref null $continuation))))
        "#
    );

    let module = Module::compile(&engine, bytes)
        .await
        .expect("a module with continuation types at its boundary compiles");
    let exports = module.exports().cloned().collect::<Vec<_>>();
    assert_eq!(
        exports[0],
        ExportType::new(
            "yield",
            ExternType::Tag(wcmp_wasm_core::TagType::new(FuncType::new([], [])))
        )
    );
    let ExternType::Func(take) = exports[1].ty() else {
        panic!("the module exports a function: {exports:?}");
    };
    let [
        ValType::Ref(RefType {
            nullable: true,
            heap: HeapType::Concrete(_),
        }),
    ] = take.params()
    else {
        panic!("the function takes a continuation reference: {take:?}");
    };

    // Wasmtime's embedding API cannot yet carry a continuation reference,
    // so a call that would pass one is a structured error, not a panic.
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let instance = Instance::instantiate(&mut store, &module, &[])
        .await
        .expect("the module instantiates");
    let take = instance
        .get_export(&mut store, "take")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports its function");
    let refused = take.call(&mut store, &[Val::ContRef(None)], &mut []);
    assert!(matches!(refused, Err(Error::Backend { .. })), "{refused:?}");
}
