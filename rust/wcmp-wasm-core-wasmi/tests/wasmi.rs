// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the Wasmi backend holds beyond the backend contract: the
//! capabilities it declares, compiles and instantiations that finish at
//! once, a refusal that names each capability it lacks where a test of the
//! contract needs one, loans of memory that copy nothing, and where
//! Wasmi sets a resumable call aside and where it does not.

#![cfg(not(target_arch = "wasm32"))]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use wcmp_macros::wasm;
use wcmp_wasm_core::{
    AnyRef, Capability, Engine, Error, ExportType, Extern, FuncType, GlobalType, I31, Instance,
    Memory, MemoryType, Module, Mutability, RefType, ResumableCall, Store, TableType, TrapKind,
    Val, ValType,
};
use wcmp_wasm_core_wasmi::Wasmi;

fn engine() -> Engine {
    Engine::with_backend(Wasmi::new())
}

/// The output of `future`, which must be ready on its first poll.
fn at_once<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future is ready on its first poll"),
    }
}

#[wcmp_macros::test]
fn it_declares_what_wasmi_implements_and_nothing_else() {
    let capabilities = engine().capabilities();

    let declared = [
        Capability::MultiMemory,
        Capability::Memory64,
        Capability::TailCall,
        Capability::RelaxedSimd,
        Capability::HostSuspension,
    ];
    for capability in Capability::ALL {
        assert_eq!(
            capabilities.contains(capability),
            declared.contains(&capability),
            "{capability}"
        );
    }
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
async fn it_describes_the_exports_in_the_order_the_module_declares_them() {
    // Wasmi keeps the exports by name, so an order that is not the order
    // of their names shows the backend reads the module's own.
    let module = Module::compile(
        &engine(),
        wasm!(
            r#"
            (module
              (func (export "zebra"))
              (memory (export "middle") 1)
              (global (export "alpha") i32 (i32.const 0)))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let names = module.exports().map(ExportType::name).collect::<Vec<_>>();
    assert_eq!(names, ["zebra", "middle", "alpha"]);
}

#[wcmp_macros::test]
async fn it_links_the_imports_in_the_order_the_module_declares_them() {
    // Wasmi takes the functions before the globals, so a global declared
    // first shows the backend puts the externs into Wasmi's order.
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "base" (global i32))
              (import "host" "double" (func $double (param i32) (result i32)))
              (func (export "run") (result i32)
                global.get 0
                call $double))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let names = module
        .imports()
        .map(|import| import.name())
        .collect::<Vec<_>>();
    assert_eq!(names, ["base", "double"]);

    let base = wcmp_wasm_core::Global::new(
        &mut store,
        GlobalType::new(ValType::I32, Mutability::Const),
        Val::I32(21),
    )
    .expect("the store makes a global");
    let double = wcmp_wasm_core::Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |_, params, results| {
            results[0] = Val::I32(params[0].i32().unwrap_or_default() * 2);
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let instance = Instance::instantiate(&mut store, &module, &[base.into(), double.into()])
        .await
        .expect("the module instantiates");
    let run = instance
        .get_export(&mut store, "run")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .expect("the instance exports its function");
    let mut result = [Val::I32(0)];
    run.call(&mut store, &[], &mut result)
        .expect("the call succeeds");
    assert_eq!(result[0].i32(), Some(42));
}

/// A module of a test of the contract that needs a capability Wasmi does
/// not declare, and the capability the refusal names.
struct Refused {
    what: &'static str,
    capability: Capability,
    bytes: &'static [u8],
}

/// The modules of the tests of the contract that need `gc`, `exceptions`,
/// or another capability Wasmi does not declare, one for each test, and
/// one for each module of the trap fixture above the floor.
const REFUSED: [Refused; 12] = [
    Refused {
        what: "a GC global and an exported tag",
        capability: Capability::Gc,
        bytes: wasm!(
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
                struct.get $box 0))
            "#
        ),
    },
    Refused {
        what: "internal items that do not cross the boundary",
        capability: Capability::Gc,
        bytes: wasm!(
            r#"
            (module
              (type $node (struct (field i32)))
              (type $callback (func (param i32) (result i32)))
              (table $callbacks 1 (ref null $callback))
              (tag $internal (param (ref null $node)))
              (global $root (mut (ref null $node)) (ref.null $node))
              (func (export "run") (param i32) (result i32)
                local.get 0))
            "#
        ),
    },
    Refused {
        what: "a tag at the boundary",
        capability: Capability::Exceptions,
        bytes: wasm!(
            r#"
            (module
              (import "peer" "fault" (tag (param i32 f64)))
              (tag (export "signal") (param i64)))
            "#
        ),
    },
    Refused {
        what: "a tag linked from one instance into another",
        capability: Capability::Exceptions,
        bytes: wasm!(
            r#"
            (module
              (tag $fault (export "fault") (param i32))
              (func (export "throw") (param i32)
                local.get 0
                throw $fault))
            "#
        ),
    },
    Refused {
        what: "an i31ref",
        capability: Capability::Gc,
        bytes: wasm!(
            r#"
            (module
              (func (export "make") (param i32) (result i31ref)
                local.get 0
                ref.i31))
            "#
        ),
    },
    Refused {
        what: "a GC object",
        capability: Capability::Gc,
        bytes: wasm!(
            r#"
            (module
              (type $pair (struct (field i32) (field i32)))
              (func (export "make") (param i32 i32) (result (ref null $pair))
                local.get 0
                local.get 1
                struct.new $pair))
            "#
        ),
    },
    Refused {
        what: "an exnref",
        capability: Capability::Exceptions,
        bytes: wasm!(
            r#"
            (module
              (func (export "nothing") (result exnref)
                ref.null exn))
            "#
        ),
    },
    Refused {
        what: "an exception that nothing catches",
        capability: Capability::Exceptions,
        bytes: wasm!(
            r#"
            (module
              (tag $oops (param i32))
              (func (export "throw") (param i32)
                local.get 0
                throw $oops))
            "#
        ),
    },
    Refused {
        what: "the trap fixture's null reference",
        capability: Capability::FunctionReferences,
        bytes: wasm!(
            r#"
            (module
              (type $nullary (func))
              (func (export "null_reference")
                ref.null $nullary
                call_ref $nullary))
            "#
        ),
    },
    Refused {
        what: "the trap fixture's arrays and casts",
        capability: Capability::Gc,
        bytes: wasm!(
            r#"
            (module
              (type $bytes (array (mut i8)))
              (func (export "array_out_of_bounds")
                i32.const 1
                array.new_default $bytes
                i32.const 1
                array.get_u $bytes
                drop))
            "#
        ),
    },
    Refused {
        what: "the trap fixture's atomics",
        capability: Capability::Threads,
        bytes: wasm!(
            r#"
            (module
              (memory 1)
              (func (export "heap_misaligned")
                i32.const 1
                i32.atomic.load
                drop))
            "#
        ),
    },
    Refused {
        what: "the trap fixture's continuations",
        capability: Capability::StackSwitching,
        bytes: wasm!(
            r#"
            (module
              (type $task (func))
              (type $continuation (cont $task))
              (tag $yield)
              (func (export "unhandled_tag")
                suspend $yield))
            "#
        ),
    },
];

#[wcmp_macros::test]
async fn it_refuses_each_module_that_needs_a_missing_capability_with_its_name() {
    let engine = engine();
    let mut failures = Vec::new();
    for refused in &REFUSED {
        let asynchronous = Module::compile(&engine, refused.bytes).await;
        let synchronous = Module::new(&engine, refused.bytes);
        for (how, outcome) in [("compile", asynchronous), ("new", synchronous)] {
            match outcome {
                Err(Error::Unsupported(capability)) if capability == refused.capability => {}
                other => failures.push(format!(
                    "{} ({how}): expected Unsupported({}), got {other:?}",
                    refused.what, refused.capability,
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[wcmp_macros::test]
fn it_refuses_each_object_that_needs_a_missing_capability_with_its_name() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");

    let i31 = AnyRef::from_i31(&mut store, I31::wrapping_i32(7));
    assert!(
        matches!(i31, Err(Error::Unsupported(Capability::Gc))),
        "{i31:?}"
    );
    let shared = Memory::new(&mut store, MemoryType::shared(1, 1));
    assert!(
        matches!(shared, Err(Error::Unsupported(Capability::Threads))),
        "{shared:?}"
    );
    let gc_global = wcmp_wasm_core::Global::new(
        &mut store,
        GlobalType::new(ValType::ANYREF, Mutability::Var),
        Val::AnyRef(None),
    );
    assert!(
        matches!(gc_global, Err(Error::Unsupported(Capability::Gc))),
        "{gc_global:?}"
    );
    let exception_table = wcmp_wasm_core::Table::new(
        &mut store,
        TableType::new(RefType::EXNREF, 1, None),
        Val::ExnRef(None),
    );
    assert!(
        matches!(
            exception_table,
            Err(Error::Unsupported(Capability::Exceptions))
        ),
        "{exception_table:?}"
    );
    let typed = wcmp_wasm_core::Func::new(
        &mut store,
        FuncType::new(
            [ValType::Ref(RefType::new(
                false,
                wcmp_wasm_core::HeapType::Func,
            ))],
            [],
        ),
        |_, _, _| Ok(()),
    );
    assert!(
        matches!(
            typed,
            Err(Error::Unsupported(Capability::FunctionReferences))
        ),
        "{typed:?}"
    );
}

#[wcmp_macros::test]
fn it_refuses_limits_wasmi_would_panic_on_with_a_structured_error() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");

    let table = wcmp_wasm_core::Table::new(
        &mut store,
        TableType::new(RefType::FUNCREF, 4, Some(2)),
        Val::FuncRef(None),
    );
    assert!(
        matches!(table, Err(Error::TypeMismatch { .. })),
        "{table:?}"
    );
    let memory = Memory::new(&mut store, MemoryType::new(4, Some(2)));
    assert!(memory.is_err(), "{memory:?}");
    let wide = wcmp_wasm_core::Func::new(
        &mut store,
        FuncType::new(vec![ValType::I32; 1_001], []),
        |_, _, _| Ok(()),
    );
    assert!(matches!(wide, Err(Error::TypeMismatch { .. })), "{wide:?}");
}

/// The addresses a loan of `len` bytes of `memory` at `offset` spans.
fn loan(store: &Store<()>, memory: Memory, offset: u64, len: usize) -> core::ops::Range<usize> {
    memory
        .with_bytes(store, offset, len, |bytes| {
            let range = bytes.as_ptr_range();
            range.start as usize..range.end as usize
        })
        .expect("the range lies inside the memory")
}

#[wcmp_macros::test]
fn it_lends_the_bytes_of_an_unshared_memory_without_a_copy() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let memory =
        Memory::new(&mut store, MemoryType::new(1, None)).expect("the store makes a memory");
    memory
        .write(&mut store, 16, &[7; 16])
        .expect("the range lies inside the memory");

    // Two loans of one memory at once are two views of the memory's own
    // bytes, as far apart as their offsets.
    let (inner, outer) = memory
        .with_bytes(&store, 0, 64, |outer| {
            let inner = loan(&store, memory, 16, 16);
            (inner, outer.as_ptr_range())
        })
        .expect("the range lies inside the memory");
    assert_eq!(inner.start, outer.start as usize + 16);

    // A later loan lends the same bytes again, and a write to the memory
    // shows through them.
    assert_eq!(loan(&store, memory, 16, 16), inner);
    memory
        .write(&mut store, 16, &[9; 16])
        .expect("the range lies inside the memory");
    let lent = memory
        .with_bytes(&store, 16, 16, |bytes| {
            (bytes.as_ptr() as usize, bytes.to_vec())
        })
        .expect("the range lies inside the memory");
    assert_eq!(lent, (inner.start, vec![9; 16]));
}

#[wcmp_macros::test]
fn it_refuses_a_memory64_maximum_whose_growth_wasmi_would_panic_on() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");

    // 2^48 pages of 64 KiB is 2^64 bytes, which overflows Wasmi's count of
    // the maximum in bytes each time the memory grows.
    let whole = Memory::new(&mut store, MemoryType::new64(0, Some(1 << 48)));
    assert!(
        matches!(whole, Err(Error::TypeMismatch { .. })),
        "{whole:?}"
    );

    // One page less fits, and the memory grows.
    let memory = Memory::new(&mut store, MemoryType::new64(0, Some((1 << 48) - 1)))
        .expect("the store makes a memory");
    assert_eq!(memory.grow(&mut store, 1).expect("the memory grows"), 0);
    assert_eq!(
        memory.size(&store).expect("the memory is the store's"),
        1 << 16
    );
}

#[wcmp_macros::test]
async fn it_refuses_to_grow_a_guest_memory_whose_growth_wasmi_would_panic_on() {
    let engine = engine();
    let mut store = Store::new(&engine, ()).expect("the engine makes a store");
    let module = Module::compile(
        &engine,
        wasm!(r#"(module (memory (export "memory") i64 0 0x1000000000000))"#),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[])
        .await
        .expect("the module instantiates");
    let memory = instance
        .get_export(&mut store, "memory")
        .expect("the instance belongs to the store")
        .and_then(Extern::into_memory)
        .expect("the instance exports its memory");

    let grown = memory.grow(&mut store, 1);
    assert!(matches!(grown, Err(Error::Grow { delta: 1 })), "{grown:?}");
    assert_eq!(memory.grow(&mut store, 0).expect("nothing grows"), 0);
}

/// What the store of the test of a descent holds: the guest function the
/// host function calls back into, how many times the host function ran,
/// and whether its deepest call back into the guest overflowed.
#[derive(Default)]
struct Descent {
    down: Option<wcmp_wasm_core::Func>,
    entered: u32,
    overflowed: bool,
}

#[wcmp_macros::test]
async fn it_traps_a_descent_through_host_functions_beyond_its_bound_with_a_stack_overflow() {
    let engine = engine();
    let mut store = Store::new(&engine, Descent::default()).expect("the engine makes a store");
    let descend = wcmp_wasm_core::Func::new(
        &mut store,
        FuncType::new([], []),
        |mut caller: wcmp_wasm_core::Caller<'_, Descent>, _, _| {
            let down = caller
                .data()
                .down
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            caller.data_mut().entered += 1;
            match down.call(&mut caller, &[], &mut []) {
                Err(Error::Trap(TrapKind::StackOverflow)) => {
                    caller.data_mut().overflowed = true;
                    anyhow::bail!("the descent overflowed")
                }
                outcome => Ok(outcome?),
            }
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        &engine,
        wasm!(
            r#"
            (module
              (import "host" "descend" (func $descend))
              (func (export "down") call $descend))
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
        .expect("the instance exports its function");
    store.data_mut().down = Some(down);

    // The host function calls back into the guest with no end, so only the
    // bound stops the descent, before the native stack runs out.
    let outcome = down.call(&mut store, &[], &mut []);
    assert!(
        matches!(outcome, Err(Error::Trap(TrapKind::Host(_)))),
        "{outcome:?}"
    );
    assert!(store.data().overflowed, "the deepest call overflowed");
    assert_eq!(store.data().entered, 64);

    // The bound counts the calls that run now, and not every call so far.
    store.data_mut().entered = 0;
    store.data_mut().overflowed = false;
    let outcome = down.call(&mut store, &[], &mut []);
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(store.data().entered, 64);
}

/// What the store of a test of where Wasmi suspends holds: the guest
/// function `through` calls back into, and the error of that call.
#[derive(Default)]
struct Between {
    inner: Option<wcmp_wasm_core::Func>,
    error: Option<Error>,
}

/// A store with a module whose functions reach the suspending host
/// function `wait` in each way Wasmi treats apart, and its instance.
///
/// `wait` answers "not yet" to every argument. `through` is a host function
/// that cannot suspend: it calls back into the guest's `inner`, which calls
/// `wait`, and keeps the error of that call in the store.
async fn waits(engine: &Engine) -> (Store<Between>, Instance) {
    let mut store = Store::new(engine, Between::default()).expect("the engine makes a store");
    let wait = wcmp_wasm_core::Func::new_suspending(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |_, _, _| Ok(Poll::Pending),
    )
    .expect("the store makes a suspending host function");
    let through = wcmp_wasm_core::Func::new(
        &mut store,
        FuncType::new([ValType::I32], [ValType::I32]),
        |mut caller: wcmp_wasm_core::Caller<'_, Between>, params, results| {
            let inner = caller
                .data()
                .inner
                .ok_or_else(|| anyhow::anyhow!("the guest function is not set"))?;
            let outcome = inner.call(&mut caller, params, results);
            caller.data_mut().error = outcome.err();
            results[0] = Val::I32(-1);
            Ok(())
        },
    )
    .expect("the store makes a host function");
    let module = Module::compile(
        engine,
        wasm!(
            r#"
            (module
              (import "host" "wait" (func $wait (param i32) (result i32)))
              (import "host" "through" (func $through (param i32) (result i32)))
              (func $below (param i32) (result i32)
                (return_call $wait (local.get 0)))
              (func (export "run") (param i32) (result i32)
                (call $wait (local.get 0)))
              (func (export "below") (param i32) (result i32)
                (call $below (local.get 0)))
              (func (export "root") (param i32) (result i32)
                (return_call $wait (local.get 0)))
              (func (export "inner") (param i32) (result i32)
                (call $wait (local.get 0)))
              (func (export "outer") (param i32) (result i32)
                (call $through (local.get 0))))
            "#
        ),
    )
    .await
    .expect("the module compiles");
    let instance = Instance::instantiate(&mut store, &module, &[wait.into(), through.into()])
        .await
        .expect("the module instantiates");
    let inner = export(&mut store, instance, "inner");
    store.data_mut().inner = Some(inner);
    (store, instance)
}

/// The function `instance` exports as `name`.
fn export(store: &mut Store<Between>, instance: Instance, name: &str) -> wcmp_wasm_core::Func {
    instance
        .get_export(store, name)
        .expect("the instance belongs to the store")
        .and_then(Extern::into_func)
        .unwrap_or_else(|| panic!("the instance exports `{name}`"))
}

/// Whether `error` is the trap of a suspension that could not suspend.
fn cannot_suspend(error: &Error) -> bool {
    matches!(
        error,
        Error::Trap(TrapKind::Host(error)) if error.to_string().contains("where its call cannot suspend")
    )
}

#[wcmp_macros::test]
async fn it_finishes_a_resumable_call_and_its_resumption_at_once() {
    let (mut store, instance) = waits(&engine()).await;
    let run = export(&mut store, instance, "run");

    let mut results = [Val::I32(0)];
    let outcome = at_once(run.call_resumable(&mut store, &[Val::I32(1)], &mut results));
    let Ok(ResumableCall::Suspended(waiting)) = outcome else {
        panic!("the call suspends: {outcome:?}");
    };
    let outcome = at_once(waiting.resume(&mut store, &[Val::I32(42)], &mut results));
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert_eq!(results[0].i32(), Some(42));
}

#[wcmp_macros::test]
async fn it_suspends_a_tail_call_below_the_root_frame_and_traps_one_from_it() {
    let (mut store, instance) = waits(&engine()).await;

    // A frame below the root tail-calls the host function, and the root
    // frame still waits for its results, so Wasmi sets the call aside.
    let below = export(&mut store, instance, "below");
    let mut results = [Val::I32(0)];
    let outcome = below
        .call_resumable(&mut store, &[Val::I32(1)], &mut results)
        .await;
    let Ok(ResumableCall::Suspended(waiting)) = outcome else {
        panic!("the call suspends: {outcome:?}");
    };
    let outcome = waiting
        .resume(&mut store, &[Val::I32(7)], &mut results)
        .await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert_eq!(results[0].i32(), Some(7));

    // The root frame tail-calls the host function, and leaves no frame to
    // resume, so Wasmi does not set the call aside.
    let root = export(&mut store, instance, "root");
    let outcome = root
        .call_resumable(&mut store, &[Val::I32(1)], &mut results)
        .await;
    assert!(
        outcome.as_ref().err().is_some_and(cannot_suspend),
        "{outcome:?}"
    );
}

#[wcmp_macros::test]
async fn it_traps_the_call_back_into_the_guest_that_a_host_frame_makes() {
    let (mut store, instance) = waits(&engine()).await;
    let outer = export(&mut store, instance, "outer");

    // The host function between the start of the call and the suspension
    // sees its own call back into the guest trap, and answers anyway, so
    // the resumable call finishes with that answer.
    let mut results = [Val::I32(0)];
    let outcome = outer
        .call_resumable(&mut store, &[Val::I32(1)], &mut results)
        .await;
    assert!(
        matches!(outcome, Ok(ResumableCall::Finished)),
        "{outcome:?}"
    );
    assert_eq!(results[0].i32(), Some(-1));
    assert!(
        store.data().error.as_ref().is_some_and(cannot_suspend),
        "{:?}",
        store.data().error
    );
}
