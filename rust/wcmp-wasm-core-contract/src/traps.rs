// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Traps: the trap fixture, and an exception that nothing catches.

use core::mem;

use wcmp_macros::wasm;
use wcmp_wasm_core::{Capability, Engine, Error, TrapKind, Val, ValType};

use crate::support;

/// One module of the trap fixture: the capabilities it needs, and one
/// exported function for each trap it raises.
struct Fixture {
    capabilities: &'static [Capability],
    bytes: &'static [u8],
    traps: &'static [Trap],
}

/// One trap of the fixture: the export that raises it, its kind, and the
/// message of the kind, which is Wasmtime's.
struct Trap {
    export: &'static str,
    kind: TrapKind,
    message: &'static str,
}

/// The trap fixture: every core trap, each raised by a function of no
/// arguments and no results, grouped by the capabilities its module needs.
///
/// The fixture leaves out the two reserved kinds, `OutOfFuel` and
/// `Interrupt`, which the lexicon's reserved capabilities would gate, and
/// which no backend declares.
const FIXTURE: [Fixture; 5] = [
    Fixture {
        capabilities: &[],
        bytes: wasm!(
            r#"
            (module
              (type $nullary (func))
              (type $unary (func (param i32) (result i32)))
              (memory 1)
              (table $functions 2 funcref)
              (elem (table $functions) (i32.const 0) func $nothing)
              (func $nothing)
              (func (export "unreachable")
                unreachable)
              (func (export "memory_out_of_bounds")
                i32.const 65536
                i32.load
                drop)
              (func (export "table_out_of_bounds")
                i32.const 2
                table.get $functions
                drop)
              (func (export "indirect_call_to_null")
                i32.const 1
                call_indirect $functions (type $nullary))
              (func (export "bad_signature")
                i32.const 7
                i32.const 0
                call_indirect $functions (type $unary)
                drop)
              (func (export "integer_overflow")
                i32.const 0x80000000
                i32.const -1
                i32.div_s
                drop)
              (func (export "integer_division_by_zero")
                i32.const 1
                i32.const 0
                i32.div_u
                drop)
              (func (export "bad_conversion_to_integer")
                f32.const nan
                i32.trunc_f32_s
                drop)
              (func $recurse (export "stack_overflow")
                call $recurse))
            "#
        ),
        traps: &[
            Trap {
                export: "unreachable",
                kind: TrapKind::UnreachableCodeReached,
                message: "wasm trap: wasm `unreachable` instruction executed",
            },
            Trap {
                export: "memory_out_of_bounds",
                kind: TrapKind::MemoryOutOfBounds,
                message: "wasm trap: out of bounds memory access",
            },
            Trap {
                export: "table_out_of_bounds",
                kind: TrapKind::TableOutOfBounds,
                message: "wasm trap: undefined element: out of bounds table access",
            },
            Trap {
                export: "indirect_call_to_null",
                kind: TrapKind::IndirectCallToNull,
                message: "wasm trap: uninitialized element",
            },
            Trap {
                export: "bad_signature",
                kind: TrapKind::BadSignature,
                message: "wasm trap: indirect call type mismatch",
            },
            Trap {
                export: "integer_overflow",
                kind: TrapKind::IntegerOverflow,
                message: "wasm trap: integer overflow",
            },
            Trap {
                export: "integer_division_by_zero",
                kind: TrapKind::IntegerDivisionByZero,
                message: "wasm trap: integer divide by zero",
            },
            Trap {
                export: "bad_conversion_to_integer",
                kind: TrapKind::BadConversionToInteger,
                message: "wasm trap: invalid conversion to integer",
            },
            Trap {
                export: "stack_overflow",
                kind: TrapKind::StackOverflow,
                message: "wasm trap: call stack exhausted",
            },
        ],
    },
    Fixture {
        capabilities: &[Capability::FunctionReferences],
        bytes: wasm!(
            r#"
            (module
              (type $nullary (func))
              (func (export "null_reference")
                ref.null $nullary
                call_ref $nullary))
            "#
        ),
        traps: &[Trap {
            export: "null_reference",
            kind: TrapKind::NullReference,
            message: "wasm trap: null reference",
        }],
    },
    Fixture {
        capabilities: &[Capability::Gc],
        bytes: wasm!(
            r#"
            (module
              (type $bytes (array (mut i8)))
              (type $words (array (mut i64)))
              (type $point (struct (field i32)))
              (func (export "array_out_of_bounds")
                i32.const 1
                array.new_default $bytes
                i32.const 1
                array.get_u $bytes
                drop)
              (func (export "allocation_too_large")
                i32.const -1
                array.new_default $words
                drop)
              (func (export "cast_failure")
                i32.const 0
                ref.i31
                ref.cast (ref $point)
                drop))
            "#
        ),
        traps: &[
            Trap {
                export: "array_out_of_bounds",
                kind: TrapKind::ArrayOutOfBounds,
                message: "wasm trap: out of bounds array access",
            },
            Trap {
                export: "allocation_too_large",
                kind: TrapKind::AllocationTooLarge,
                message: "wasm trap: allocation size too large",
            },
            Trap {
                export: "cast_failure",
                kind: TrapKind::CastFailure,
                message: "wasm trap: cast failure",
            },
        ],
    },
    Fixture {
        capabilities: &[Capability::Threads],
        bytes: wasm!(
            r#"
            (module
              (memory 1)
              (func (export "heap_misaligned")
                i32.const 1
                i32.atomic.load
                drop)
              (func (export "atomic_wait_non_shared_memory")
                i32.const 0
                i32.const 0
                i64.const 0
                memory.atomic.wait32
                drop))
            "#
        ),
        traps: &[
            Trap {
                export: "heap_misaligned",
                kind: TrapKind::HeapMisaligned,
                message: "wasm trap: unaligned atomic",
            },
            Trap {
                export: "atomic_wait_non_shared_memory",
                kind: TrapKind::AtomicWaitNonSharedMemory,
                message: "wasm trap: atomic wait on non-shared memory",
            },
        ],
    },
    Fixture {
        capabilities: &[Capability::StackSwitching],
        bytes: wasm!(
            r#"
            (module
              (type $task (func))
              (type $continuation (cont $task))
              (tag $yield)
              (func $finish)
              (elem declare func $finish)
              (func (export "unhandled_tag")
                suspend $yield)
              (func (export "continuation_already_consumed")
                (local $task (ref null $continuation))
                ref.func $finish
                cont.new $continuation
                local.tee $task
                resume $continuation
                local.get $task
                resume $continuation))
            "#
        ),
        traps: &[
            Trap {
                export: "unhandled_tag",
                kind: TrapKind::UnhandledTag,
                message: "wasm trap: unhandled tag",
            },
            Trap {
                export: "continuation_already_consumed",
                kind: TrapKind::ContinuationAlreadyConsumed,
                message: "wasm trap: continuation already consumed",
            },
        ],
    },
];

/// The trap fixture raises each core trap that the engine's capabilities
/// permit, and the engine reports each with its kind and Wasmtime's
/// message.
///
/// A module of the fixture whose capabilities the engine does not declare
/// does not run. Each trap runs in a store of its own, so one trap cannot
/// leave a store in a state that changes the next.
pub async fn it_raises_each_core_trap_the_capabilities_permit(engine: &Engine) {
    it_raises_each_core_trap_allowing(engine, &[]).await;
}

/// A trap of the fixture whose engine words it with a message that names
/// more than one kind, so that the backend reports [`TrapKind::Other`] with
/// that message rather than a kind that could be wrong.
pub struct AmbiguousTrap {
    /// The export of the fixture that raises the trap.
    pub export: &'static str,
    /// The engine's message for the trap, which the backend reports as it
    /// is.
    pub message: &'static str,
}

/// [`it_raises_each_core_trap_the_capabilities_permit`], where each trap of
/// `ambiguous` may come back as [`TrapKind::Other`] with its engine's
/// message instead of its kind.
///
/// An allowance holds only while it is needed. Where the backend reports
/// the trap's own kind with Wasmtime's message, the allowance is stale and
/// the test fails, so that it goes away once the engine words the trap
/// apart. An allowance that names no export of the fixture fails too.
pub async fn it_raises_each_core_trap_allowing(engine: &Engine, ambiguous: &[AmbiguousTrap]) {
    let mut failures = Vec::new();
    for allowance in ambiguous {
        if !FIXTURE
            .iter()
            .flat_map(|fixture| fixture.traps)
            .any(|trap| trap.export == allowance.export)
        {
            failures.push(format!(
                "`{}`: the allowance names no trap of the fixture",
                allowance.export
            ));
        }
    }
    let mut raised = 0;
    for fixture in FIXTURE
        .iter()
        .filter(|fixture| support::declares(engine, fixture.capabilities))
    {
        for trap in fixture.traps {
            let mut store = support::store(engine, ());
            let instance = support::instance(&mut store, fixture.bytes, &[]).await;
            let raise = support::func(&mut store, instance, trap.export);
            let allowance = ambiguous
                .iter()
                .find(|allowance| allowance.export == trap.export);
            match (raise.call(&mut store, &[], &mut []), allowance) {
                (Err(Error::Trap(kind)), None)
                    if mem::discriminant(&kind) == mem::discriminant(&trap.kind)
                        && kind.to_string() == trap.message => {}
                (Err(Error::Trap(TrapKind::Other(message))), Some(allowance))
                    if message == allowance.message => {}
                (Err(Error::Trap(kind)), Some(allowance))
                    if mem::discriminant(&kind) == mem::discriminant(&trap.kind)
                        && kind.to_string() == trap.message =>
                {
                    failures.push(format!(
                        "`{}`: stale allowance: the backend reports {:?} with {:?}, so the \
                         engine no longer words the trap as {:?}; remove the allowance",
                        trap.export, trap.kind, trap.message, allowance.message,
                    ));
                }
                (other, allowance) => failures.push(format!(
                    "`{}`: expected {:?} with {:?}{}, got {other:?}{}",
                    trap.export,
                    trap.kind,
                    trap.message,
                    match allowance {
                        Some(allowance) => format!(" or Other with {:?}", allowance.message),
                        None => String::new(),
                    },
                    match &other {
                        Err(error) => format!(" with {:?}", error.to_string()),
                        Ok(()) => String::new(),
                    },
                )),
            }
            raised += 1;
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(raised >= 9, "the floor alone raises nine traps");
}

/// A guest throws an exception that nothing catches. The call fails with
/// [`TrapKind::UncaughtException`] and the message `thrown Wasm exception`.
/// The kind carries the exception as an opaque reference, which the host
/// gives back to a guest of the same store, and the guest reads its
/// payload.
pub async fn it_fails_with_an_exception_that_nothing_catches(engine: &Engine) {
    if !support::declares(engine, &[Capability::Exceptions]) {
        return;
    }
    let mut store = support::store(engine, ());
    let instance = support::instance(
        &mut store,
        wasm!(
            r#"
            (module
              (tag $oops (param i32))
              (func (export "throw") (param i32)
                local.get 0
                throw $oops)
              (func (export "payload") (param exnref) (result i32)
                block $caught (result i32)
                  try_table (catch $oops $caught)
                    local.get 0
                    throw_ref
                  end
                  unreachable
                end))
            "#
        ),
        &[],
    )
    .await;
    let throw = support::func(&mut store, instance, "throw");
    let payload = support::func(&mut store, instance, "payload");

    let exception = match throw.call(&mut store, &[Val::I32(7)], &mut []) {
        Err(Error::Trap(kind @ TrapKind::UncaughtException(_))) => {
            assert_eq!(kind.to_string(), "thrown Wasm exception");
            let TrapKind::UncaughtException(exception) = kind else {
                unreachable!("the kind was matched above");
            };
            exception
        }
        other => panic!("the call fails with an uncaught exception: {other:?}"),
    };

    let read = support::call(
        &mut store,
        payload,
        &[Val::ExnRef(Some(exception))],
        &[ValType::I32],
    );
    assert_eq!(
        read[0].i32(),
        Some(7),
        "the guest reads the exception's payload"
    );
}
