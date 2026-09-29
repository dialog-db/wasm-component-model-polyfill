//! The engine's own plumbing, over test doubles of a backend: dynamic
//! dispatch to two backends in one binary, the checks the engine makes
//! before it reaches a backend, host functions, memory access, and the
//! capabilities a method needs.

mod double;

use std::any::Any;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use double::{Double, Minimal, Refuser};
use wcmp_wasm_core::backend::{BackendSuspendedCall, RawHandle};
use wcmp_wasm_core::{
    AnyRef, Capabilities, Capability, Engine, Error, Extern, ExternRef, Func, FuncType, Global,
    GlobalType, I31, Instance, Memory, MemoryType, Module, Mutability, RefType, ResumableCall,
    Store, SuspendedCall, Table, TableType, TrapKind, Val, ValType,
};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine(capabilities: Capabilities) -> Engine {
    Engine::with_backend(Double { capabilities })
}

fn store<T: Send + 'static>(engine: &Engine, data: T) -> Store<T> {
    Store::new(engine, data).expect("the double makes a store")
}

fn memory(store: &mut Store<()>) -> Memory {
    Memory::new(store, MemoryType::new(1, Some(2))).expect("the double makes a memory")
}

/// The first call of `store` that suspends in `func`.
async fn suspend<T: 'static>(func: Func, store: &mut Store<T>) -> SuspendedCall {
    let outcome = func
        .call_resumable(store, &[], &mut [Val::I32(0)])
        .await
        .expect("the call suspends");
    let ResumableCall::Suspended(call) = outcome else {
        panic!("the call suspends: {outcome:?}");
    };
    call
}

/// The output of `future`, which the double finishes on its first poll.
/// A host function is synchronous, so it cannot await a resumption.
fn now<F: Future>(future: F) -> Option<F::Output> {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

/// A waiting call of no backend's making.
struct Stray;

impl BackendSuspendedCall for Stray {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

#[wcmp_macros::test]
async fn it_holds_engines_over_two_backends_in_one_binary() {
    let engines = [
        engine(Capabilities::empty().with(Capability::TailCall)),
        Engine::with_backend(Refuser),
    ];

    assert!(engines[0].capabilities().contains(Capability::TailCall));
    assert_eq!(engines[1].capabilities(), Capabilities::empty());
    assert!(!Engine::same(&engines[0], &engines[1]));
    assert!(Engine::same(&engines[0], &engines[0].clone()));

    let compiled = Module::compile(&engines[0], b"imports 0").await;
    assert!(compiled.is_ok(), "the double compiles: {compiled:?}");
    let refused = Module::compile(&engines[1], b"imports 0").await;
    assert!(
        matches!(&refused, Err(Error::Compile { message }) if message == "the refuser compiles nothing"),
        "the refuser answers for itself: {refused:?}"
    );
    assert!(matches!(
        Store::new(&engines[1], ()),
        Err(Error::Backend { .. })
    ));
}

#[wcmp_macros::test]
async fn it_describes_the_boundary_the_backend_reports() {
    let engine = engine(Capabilities::empty());
    let module = Module::new(&engine, b"imports 2").expect("the double compiles");
    let imports: Vec<_> = module
        .imports()
        .map(|import| (import.module().to_string(), import.name().to_string()))
        .collect();
    assert_eq!(
        imports,
        [
            ("host".to_string(), "m0".to_string()),
            ("host".to_string(), "m1".to_string())
        ]
    );
    assert_eq!(module.exports().len(), 2);
    assert!(Engine::same(module.engine(), &engine));
}

#[wcmp_macros::test]
async fn it_instantiates_with_an_ordered_list_of_imports() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let first = memory(&mut store);
    let second = memory(&mut store);
    second
        .store_u8(&mut store, 0, 7)
        .expect("the store is in bounds");
    let module = Module::compile(&engine, b"imports 2")
        .await
        .expect("the double compiles");

    let instance = Instance::instantiate(
        &mut store,
        &module,
        &[Extern::Memory(first), Extern::Memory(second)],
    )
    .await
    .expect("the double instantiates");

    let exported = instance
        .get_export(&mut store, "m1")
        .expect("the instance is the store's")
        .and_then(Extern::into_memory)
        .expect("m1 is a memory");
    assert_eq!(exported.load_u8(&store, 0).expect("in bounds"), 7);
    assert!(
        instance
            .get_export(&mut store, "missing")
            .expect("the instance is the store's")
            .is_none()
    );
}

#[wcmp_macros::test]
async fn it_counts_the_imports_of_an_instantiation() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let module = Module::new(&engine, b"imports 2").expect("the double compiles");
    let only = memory(&mut store);
    let result = Instance::instantiate(&mut store, &module, &[Extern::Memory(only)]).await;
    assert!(
        matches!(
            result,
            Err(Error::ImportCount {
                expected: 2,
                actual: 1
            })
        ),
        "{result:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_a_module_of_another_engine() {
    let one = engine(Capabilities::empty());
    let other = engine(Capabilities::empty());
    let module = Module::new(&one, b"imports 0").expect("the double compiles");
    let mut store = store(&other, ());
    let result = Instance::instantiate(&mut store, &module, &[]).await;
    assert!(matches!(result, Err(Error::WrongEngine)), "{result:?}");
}

#[wcmp_macros::test]
async fn it_refuses_a_handle_of_another_store() {
    let engine = engine(Capabilities::empty());
    let mut one = store(&engine, ());
    let mut other = store(&engine, ());
    let foreign = memory(&mut one);
    let own = memory(&mut other);

    assert!(matches!(foreign.size(&other), Err(Error::WrongStore)));
    assert!(matches!(
        foreign.read(&other, 0, &mut [0; 4]),
        Err(Error::WrongStore)
    ));
    assert!(matches!(
        foreign.write(&mut other, 0, &[1]),
        Err(Error::WrongStore)
    ));
    assert!(matches!(
        Memory::copy(&mut other, &foreign, 0, &own, 0, 1),
        Err(Error::WrongStore)
    ));

    let module = Module::new(&engine, b"imports 1").expect("the double compiles");
    let result = Instance::instantiate(&mut other, &module, &[Extern::Memory(foreign)]).await;
    assert!(matches!(result, Err(Error::WrongStore)), "{result:?}");

    let func = Func::new(&mut one, FuncType::new([], []), |_, _, _| Ok(()))
        .expect("the double makes a function");
    assert!(matches!(
        func.call(&mut other, &[], &mut []),
        Err(Error::WrongStore)
    ));
    let own_func = Func::new(
        &mut other,
        FuncType::new([ValType::FUNCREF], []),
        |_, _, _| Ok(()),
    )
    .expect("the double makes a function");
    assert!(
        matches!(
            own_func.call(&mut other, &[Val::FuncRef(Some(func))], &mut []),
            Err(Error::WrongStore)
        ),
        "a reference among the arguments belongs to the other store"
    );
}

#[wcmp_macros::test]
fn it_calls_a_host_function_that_reaches_the_store() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, 0_u32);
    let add = Func::new(
        &mut store,
        FuncType::new([ValType::I32, ValType::I32], [ValType::I32]),
        |mut caller, params, results| {
            *caller.data_mut() += 1;
            let sum = params[0].i32().unwrap_or(0) + params[1].i32().unwrap_or(0);
            results[0] = Val::I32(sum);
            Ok(())
        },
    )
    .expect("the double makes a function");

    let mut results = [Val::I32(0)];
    add.call(&mut store, &[Val::I32(2), Val::I32(3)], &mut results)
        .expect("the call returns");
    assert_eq!(results[0].i32(), Some(5));
    assert_eq!(*store.data(), 1);
    assert_eq!(
        add.ty(&store).expect("the function is the store's"),
        Some(FuncType::new([ValType::I32, ValType::I32], [ValType::I32]))
    );
}

#[wcmp_macros::test]
fn it_enters_a_host_function_again_at_any_depth() {
    let engine = engine(Capabilities::empty());
    let mut store: Store<Option<Func>> = store(&engine, None);
    // Each depth adds its own argument to what the depth below returns, so
    // the sum is right only if no depth shares its results with another.
    let sum = Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |mut caller, params, results| {
            let n = params[0].i32().unwrap_or(0);
            let below = if n == 0 {
                0
            } else {
                let this = caller
                    .data()
                    .ok_or_else(|| anyhow::anyhow!("no function"))?;
                let mut inner = [Val::I32(0)];
                this.call(&mut caller, &[Val::I32(n - 1)], &mut inner)?;
                inner[0].i32().unwrap_or(0)
            };
            results[0] = Val::I32(n + below);
            Ok(())
        },
    )
    .expect("the double makes a function");
    *store.data_mut() = Some(sum);

    let mut results = [Val::I32(0)];
    sum.call(&mut store, &[Val::I32(5)], &mut results)
        .expect("the call returns");
    assert_eq!(results[0].i32(), Some(15));
}

#[wcmp_macros::test]
fn it_carries_a_host_error_back_unchanged() {
    #[derive(Debug, thiserror::Error)]
    #[error("the host is out of widgets")]
    struct OutOfWidgets;

    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let fails = Func::new(&mut store, FuncType::new([], []), |_, _, _| {
        Err(OutOfWidgets.into())
    })
    .expect("the double makes a function");

    let error = fails
        .call(&mut store, &[], &mut [])
        .expect_err("the host function fails");
    assert_eq!(error.to_string(), "the host is out of widgets");
    let Error::Trap(TrapKind::Host(host)) = error else {
        panic!("a host error is a `Host` trap: {error:?}");
    };
    assert!(host.downcast_ref::<OutOfWidgets>().is_some());
}

#[wcmp_macros::test]
fn it_reads_and_writes_memory_through_the_backend() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let memory = memory(&mut store);

    assert_eq!(memory.size(&store).expect("size"), MemoryType::PAGE_SIZE);
    memory
        .write(&mut store, 8, &[1, 2, 3, 4, 5, 6, 7, 8])
        .expect("in bounds");
    assert_eq!(memory.load_u8(&store, 8).expect("in bounds"), 1);
    assert_eq!(memory.load_u16(&store, 8).expect("in bounds"), 0x0201);
    assert_eq!(memory.load_u32(&store, 8).expect("in bounds"), 0x0403_0201);
    assert_eq!(
        memory.load_u64(&store, 8).expect("in bounds"),
        0x0807_0605_0403_0201
    );
    memory
        .store_u32(&mut store, 16, 0xdead_beef)
        .expect("in bounds");
    let mut buffer = [0; 4];
    memory.read(&store, 16, &mut buffer).expect("in bounds");
    assert_eq!(buffer, 0xdead_beef_u32.to_le_bytes());

    let lent = memory
        .with_bytes(&store, 9, 3, <[u8]>::to_vec)
        .expect("in bounds");
    assert_eq!(lent, [2, 3, 4]);

    assert_eq!(memory.grow(&mut store, 1).expect("below the maximum"), 1);
    assert_eq!(
        memory.size(&store).expect("size"),
        2 * MemoryType::PAGE_SIZE
    );
    assert!(matches!(
        memory.grow(&mut store, 1),
        Err(Error::Grow { delta: 1 })
    ));
}

#[wcmp_macros::test]
fn it_copies_between_two_memories_of_one_store() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let source = memory(&mut store);
    let destination = memory(&mut store);
    source.write(&mut store, 100, b"hello").expect("in bounds");

    Memory::copy(&mut store, &source, 100, &destination, 3, 5).expect("in bounds");

    let mut buffer = [0; 5];
    destination.read(&store, 3, &mut buffer).expect("in bounds");
    assert_eq!(&buffer, b"hello");
}

#[wcmp_macros::test]
fn it_reports_a_range_outside_the_memory_as_an_error() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let memory = memory(&mut store);
    let last = MemoryType::PAGE_SIZE - 1;
    let outside = |error| matches!(error, Error::MemoryOutOfBounds { .. });

    assert!(outside(memory.load_u16(&store, last).expect_err("outside")));
    assert!(outside(
        memory.load_u32(&store, u64::MAX).expect_err("outside")
    ));
    assert!(outside(
        memory.store_u64(&mut store, last, 0).expect_err("outside")
    ));
    assert!(outside(
        memory.read(&store, last, &mut [0; 2]).expect_err("outside")
    ));
    assert!(outside(
        memory
            .write(&mut store, last, &[0; 2])
            .expect_err("outside")
    ));
    assert!(outside(
        memory
            .with_bytes(&store, last, 2, |_| ())
            .expect_err("outside")
    ));
    assert!(outside(
        Memory::copy(&mut store, &memory, last, &memory, 0, 2).expect_err("outside")
    ));
    let error = memory.load_u16(&store, last).expect_err("outside");
    assert_eq!(
        error.to_string(),
        "2 bytes at offset 65535 fall outside the memory of 65536 bytes"
    );
}

#[wcmp_macros::test]
fn it_returns_unsupported_for_a_capability_the_backend_lacks() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let unsupported = |error, capability| matches!(error, Error::Unsupported(c) if c == capability);

    assert!(unsupported(
        Memory::new(&mut store, MemoryType::new64(1, None)).expect_err("no memory64"),
        Capability::Memory64
    ));
    assert!(unsupported(
        Memory::new(&mut store, MemoryType::shared(1, 1)).expect_err("no threads"),
        Capability::Threads
    ));
    assert!(unsupported(
        Table::new(
            &mut store,
            TableType::new64(RefType::FUNCREF, 1, None),
            Val::FuncRef(None)
        )
        .expect_err("no memory64"),
        Capability::Memory64
    ));
    assert!(unsupported(
        Global::new(
            &mut store,
            GlobalType::new(ValType::ANYREF, Mutability::Var),
            Val::AnyRef(None)
        )
        .expect_err("no gc"),
        Capability::Gc
    ));
    assert!(unsupported(
        Func::new(
            &mut store,
            FuncType::new([ValType::EXNREF], []),
            |_, _, _| Ok(())
        )
        .expect_err("no exceptions"),
        Capability::Exceptions
    ));
    assert!(unsupported(
        Func::new_suspending(&mut store, FuncType::new([], []), |_, _, _| Ok(
            Poll::Pending
        ))
        .expect_err("no host suspension"),
        Capability::HostSuspension
    ));
    assert!(unsupported(
        AnyRef::from_i31(&mut store, I31::wrapping_i32(1)).expect_err("no gc"),
        Capability::Gc
    ));
    let memory = memory(&mut store);
    let any_ref = AnyRef::from_raw(memory.store_id(), 0);
    assert!(unsupported(
        any_ref.as_i31(&store).expect_err("no gc"),
        Capability::Gc
    ));
}

#[wcmp_macros::test]
async fn it_returns_unsupported_for_a_resumable_call_without_host_suspension() {
    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let func = Func::new(&mut store, FuncType::new([], []), |_, _, _| Ok(()))
        .expect("the double makes a function");
    let result = func.call_resumable(&mut store, &[], &mut []).await;
    assert!(
        matches!(result, Err(Error::Unsupported(Capability::HostSuspension))),
        "{result:?}"
    );

    let call = SuspendedCall::new(func.store_id(), Box::new(Stray));
    let result = call.resume(&mut store, &[], &mut []).await;
    assert!(
        matches!(result, Err(Error::Unsupported(Capability::HostSuspension))),
        "{result:?}"
    );
}

/// `Minimal` declares every capability its methods need, and leaves each
/// default body in place, so every one of them answers `Unsupported`.
#[wcmp_macros::test]
async fn it_answers_unsupported_from_the_default_bodies_of_a_backend() {
    let engine = Engine::with_backend(Minimal {
        capabilities: Capabilities::empty()
            .with(Capability::HostSuspension)
            .with(Capability::Gc),
    });
    let mut store = store(&engine, ());
    let unsupported = |error, capability| matches!(error, Error::Unsupported(c) if c == capability);

    let func = Func::new(&mut store, FuncType::new([], []), |_, _, _| Ok(()))
        .expect("the double makes a function");
    assert!(unsupported(
        func.call_resumable(&mut store, &[], &mut [])
            .await
            .expect_err("the default body"),
        Capability::HostSuspension
    ));
    let call = SuspendedCall::new(func.store_id(), Box::new(Stray));
    assert!(unsupported(
        call.resume(&mut store, &[], &mut [])
            .await
            .expect_err("the default body"),
        Capability::HostSuspension
    ));
    assert!(unsupported(
        AnyRef::from_i31(&mut store, I31::wrapping_i32(1)).expect_err("the default body"),
        Capability::Gc
    ));
    assert!(unsupported(
        AnyRef::from_raw(func.store_id(), 0)
            .as_i31(&store)
            .expect_err("the default body"),
        Capability::Gc
    ));
}

#[wcmp_macros::test]
fn it_reports_a_backend_that_lends_no_bytes_as_an_error() {
    let engine = Engine::with_backend(Minimal {
        capabilities: Capabilities::empty(),
    });
    let mut store = store(&engine, ());
    let memory = memory(&mut store);
    let mut called = false;
    let result = memory.with_bytes(&store, 0, 4, |_| called = true);
    assert!(
        matches!(&result, Err(Error::Backend { message }) if message == "the backend did not lend the bytes of the memory"),
        "{result:?}"
    );
    assert!(!called);
}

#[wcmp_macros::test]
async fn it_resumes_waiting_calls_in_any_order() {
    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    let mut store = store(&engine, ());
    let waits = Func::new_suspending(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
        Ok(Poll::Pending)
    })
    .expect("the double declares host suspension");

    let mut waiting = Vec::new();
    for _ in 0..3 {
        let outcome = waits
            .call_resumable(&mut store, &[], &mut [Val::I32(0)])
            .await
            .expect("the call suspends");
        let ResumableCall::Suspended(call) = outcome else {
            panic!("the call suspends: {outcome:?}");
        };
        waiting.push(call);
    }

    // Third, first, second: each call finishes with its own results.
    let mut calls: Vec<_> = waiting.into_iter().map(Some).collect();
    for (index, value) in [(2, 30), (0, 10), (1, 20)] {
        let call = calls[index].take().expect("each call resumes once");
        let mut results = [Val::I32(0)];
        let outcome = call
            .resume(&mut store, &[Val::I32(value)], &mut results)
            .await
            .expect("the call resumes");
        assert!(matches!(outcome, ResumableCall::Finished), "{outcome:?}");
        assert_eq!(results[0].i32(), Some(value));
    }
}

#[wcmp_macros::test]
async fn it_refuses_to_resume_a_call_with_another_store() {
    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    let mut one = store(&engine, ());
    let mut other = store(&engine, ());
    let waits = Func::new_suspending(&mut one, FuncType::new([], []), |_, _, _| Ok(Poll::Pending))
        .expect("the double declares host suspension");
    let ResumableCall::Suspended(call) = waits
        .call_resumable(&mut one, &[], &mut [])
        .await
        .expect("the call suspends")
    else {
        panic!("the call suspends");
    };
    let result = call.resume(&mut other, &[], &mut []).await;
    assert!(matches!(result, Err(Error::WrongStore)), "{result:?}");
}

#[wcmp_macros::test]
async fn it_resumes_a_call_from_inside_a_host_function() {
    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    // The waiting call is not `Send` in the browser, so the store is made
    // here rather than through `store`, which asks for `Send`.
    let mut store: Store<Option<SuspendedCall>> =
        Store::new(&engine, None).expect("the double makes a store");
    let waits = Func::new_suspending(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
        Ok(Poll::Pending)
    })
    .expect("the double declares host suspension");
    let call = suspend(waits, &mut store).await;
    *store.data_mut() = Some(call);

    // The host function receives the double's own caller type, not its
    // store, and resumes the call through it.
    let resumes = Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |mut caller, params, results| {
            let call = caller
                .data_mut()
                .take()
                .ok_or_else(|| anyhow::anyhow!("no call waits"))?;
            let outcome = now(call.resume(&mut caller, params, results))
                .ok_or_else(|| anyhow::anyhow!("the resumption did not finish at once"))??;
            anyhow::ensure!(
                matches!(outcome, ResumableCall::Finished),
                "the call finishes: {outcome:?}"
            );
            Ok(())
        },
    )
    .expect("the double makes a function");

    let mut results = [Val::I32(0)];
    resumes
        .call(&mut store, &[Val::I32(42)], &mut results)
        .expect("the host function resumes the call");
    assert_eq!(results[0].i32(), Some(42));
    assert!(store.data().is_none());
}

#[wcmp_macros::test]
async fn it_checks_a_resumption_against_the_state_the_store_keeps() {
    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    let mut store = store(&engine, ());
    let waits = Func::new_suspending(&mut store, FuncType::new([], [ValType::I32]), |_, _, _| {
        Ok(Poll::Pending)
    })
    .expect("the double declares host suspension");
    let call = suspend(waits, &mut store).await;

    // The call holds only the slot of its state. The store knows the
    // function it waits in has one result, and refuses two.
    let result = call
        .resume(&mut store, &[Val::I32(1), Val::I32(2)], &mut [Val::I32(0)])
        .await;
    assert!(
        matches!(&result, Err(Error::TypeMismatch { message }) if message == "the resumption gave 2 results to a function of 1"),
        "{result:?}"
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_resume_a_call_the_backend_did_not_make() {
    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    let mut store = store(&engine, ());
    let memory = memory(&mut store);
    let call = SuspendedCall::new(memory.store_id(), Box::new(Stray));
    let result = call.resume(&mut store, &[], &mut []).await;
    assert!(matches!(result, Err(Error::WrongStore)), "{result:?}");
}

#[wcmp_macros::test]
fn it_hands_back_the_value_of_an_externref_and_the_integer_of_an_i31ref() {
    let engine = engine(Capabilities::empty().with(Capability::Gc));
    let mut store = store(&engine, ());
    let extern_ref = ExternRef::new(&mut store, "a host value").expect("the double makes one");
    let data = extern_ref
        .data(&store)
        .expect("the reference is the store's");
    assert_eq!(data.downcast_ref::<&str>(), Some(&"a host value"));

    let i31 = AnyRef::from_i31(&mut store, I31::wrapping_i32(-7)).expect("the double makes one");
    assert_eq!(
        i31.as_i31(&store)
            .expect("the reference is the store's")
            .map(I31::get_i32),
        Some(-7)
    );
}

#[wcmp_macros::test]
fn it_words_an_uncaught_exception_as_wasmtime_does() {
    use wcmp_wasm_core::ExnRef;
    use wcmp_wasm_core::backend::RawHandle;

    let engine = engine(Capabilities::empty());
    let mut store = store(&engine, ());
    let memory = memory(&mut store);
    // Wasmtime words an uncaught exception as its `ThrownException` does.
    let exception = ExnRef::from_raw(memory.store_id(), 0);
    let error = Error::from(TrapKind::UncaughtException(exception));
    assert_eq!(error.to_string(), "thrown Wasm exception");
}

/// Natively, an engine, a module, and a store move between threads, as
/// Wasmtime's do, and so do the futures of a compile, an instantiation, and
/// a resumable call.
#[cfg(not(target_arch = "wasm32"))]
#[wcmp_macros::test]
fn it_moves_engines_modules_and_stores_between_threads() {
    fn send<T: Send>(_: &T) {}
    fn sync<T: Sync>(_: &T) {}

    let engine = engine(Capabilities::empty().with(Capability::HostSuspension));
    let module = Module::new(&engine, b"imports 0").expect("the double compiles");
    let mut store = store(&engine, ());
    let func = Func::new(&mut store, FuncType::new([], []), |_, _, _| Ok(()))
        .expect("the double makes a function");
    send(&engine);
    sync(&engine);
    send(&module);
    sync(&module);
    send(&store);
    send(&Module::compile(&engine, b"imports 0"));
    send(&Instance::instantiate(&mut store, &module, &[]));
    send(&func.call_resumable(&mut store, &[], &mut []));
}
