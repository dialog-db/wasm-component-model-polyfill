// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the Wasmtime backend holds beyond the backend contract: the
//! capabilities it declares, compiles and instantiations that finish at
//! once, continuation types at the boundary, and loans of memory that copy
//! nothing.

#![cfg(not(target_arch = "wasm32"))]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    Capability, Engine, Error, ExportType, Extern, ExternType, FuncType, HeapType, Instance,
    Memory, MemoryType, Module, RefType, Store, Val, ValType,
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

/// Where two nested loans of one memory lie: the address of the inner
/// loan's first byte, and the addresses the outer loan spans.
fn nested_loans(store: &Store<()>, memory: Memory) -> (usize, core::ops::Range<usize>) {
    memory
        .with_bytes(store, 0, 64, |outer| {
            let outer = outer.as_ptr_range();
            let inner = memory
                .with_bytes(store, 16, 16, |inner| inner.as_ptr() as usize)
                .expect("the range lies inside the memory");
            (inner, outer.start as usize..outer.end as usize)
        })
        .expect("the range lies inside the memory")
}

#[wcmp_macros::test]
fn it_lends_the_bytes_of_an_unshared_memory_without_a_copy() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let memory =
        Memory::new(&mut store, MemoryType::new(1, None)).expect("the store makes a memory");

    // Two loans of one memory at once are two views of the memory's own
    // bytes, as far apart as their offsets.
    let (inner, outer) = nested_loans(&store, memory);
    assert_eq!(inner, outer.start + 16);
}

#[wcmp_macros::test]
fn it_lends_a_copy_of_a_shared_memory() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let memory =
        Memory::new(&mut store, MemoryType::shared(1, 1)).expect("the store makes a memory");
    memory
        .write(&mut store, 16, &[7; 16])
        .expect("the range lies inside the memory");

    // Each loan of a shared memory is a copy of its own, so the inner loan
    // lies outside the outer one.
    let (inner, outer) = nested_loans(&store, memory);
    assert!(
        !outer.contains(&inner),
        "{inner:#x} lies outside {outer:#x?}"
    );
    let copied = memory
        .with_bytes(&store, 16, 16, <[u8]>::to_vec)
        .expect("the range lies inside the memory");
    assert_eq!(copied, [7; 16]);
}

#[wcmp_macros::test]
fn it_keeps_one_handle_for_a_function_that_crosses_the_boundary_again() {
    // A guest that hands the host the same function on every call, and an
    // export the host looks up again, each name one function: the store
    // keeps one slot for it rather than one per crossing.
    use wcmp_wasm_core::backend::RawHandle;

    let engine = engine();
    let bytes = wasm!(
        r#"
        (module
          (func $triple (param i32) (result i32)
            local.get 0
            i32.const 3
            i32.mul)
          (elem declare func $triple)
          (func (export "get") (result funcref)
            ref.func $triple))
        "#
    );
    let module = at_once(Module::compile(&engine, bytes)).expect("the module compiles");
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let instance =
        at_once(Instance::instantiate(&mut store, &module, &[])).expect("the module instantiates");
    let get = |store: &mut Store<()>| {
        instance
            .get_export(store, "get")
            .expect("the instance belongs to the store")
            .and_then(Extern::into_func)
            .expect("the instance exports `get`")
    };

    let first_get = get(&mut store);
    assert_eq!(get(&mut store).index(), first_get.index());

    let mut handed = Vec::new();
    for _ in 0..8 {
        let mut result = [Val::FuncRef(None)];
        first_get
            .call(&mut store, &[], &mut result)
            .expect("the call succeeds");
        let Val::FuncRef(Some(triple)) = result[0] else {
            panic!("the guest hands out a funcref: {result:?}");
        };
        handed.push(triple.index());
    }
    assert!(
        handed.iter().all(|index| *index == handed[0]),
        "every crossing of the one function has its first handle: {handed:?}"
    );
}
