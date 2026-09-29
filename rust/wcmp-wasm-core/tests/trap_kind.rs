//! The trap kinds carry Wasmtime's names and messages.

use wasmtime_environ::Trap;
use wcmp_wasm_core::{Error, TrapKind};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Each core trap kind, beside the Wasmtime trap of the same name. The list
/// is the list of core traps the runtime layer's design names, with the
/// two reserved kinds last.
fn kinds() -> [(TrapKind, Trap); 19] {
    [
        (
            TrapKind::UnreachableCodeReached,
            Trap::UnreachableCodeReached,
        ),
        (TrapKind::MemoryOutOfBounds, Trap::MemoryOutOfBounds),
        (TrapKind::TableOutOfBounds, Trap::TableOutOfBounds),
        (TrapKind::IndirectCallToNull, Trap::IndirectCallToNull),
        (TrapKind::BadSignature, Trap::BadSignature),
        (TrapKind::IntegerOverflow, Trap::IntegerOverflow),
        (TrapKind::IntegerDivisionByZero, Trap::IntegerDivisionByZero),
        (
            TrapKind::BadConversionToInteger,
            Trap::BadConversionToInteger,
        ),
        (TrapKind::StackOverflow, Trap::StackOverflow),
        (TrapKind::NullReference, Trap::NullReference),
        (TrapKind::ArrayOutOfBounds, Trap::ArrayOutOfBounds),
        (TrapKind::AllocationTooLarge, Trap::AllocationTooLarge),
        (TrapKind::CastFailure, Trap::CastFailure),
        (TrapKind::UnhandledTag, Trap::UnhandledTag),
        (
            TrapKind::ContinuationAlreadyConsumed,
            Trap::ContinuationAlreadyConsumed,
        ),
        (TrapKind::HeapMisaligned, Trap::HeapMisaligned),
        (
            TrapKind::AtomicWaitNonSharedMemory,
            Trap::AtomicWaitNonSharedMemory,
        ),
        (TrapKind::OutOfFuel, Trap::OutOfFuel),
        (TrapKind::Interrupt, Trap::Interrupt),
    ]
}

#[wcmp_macros::test]
fn it_words_each_core_trap_as_wasmtime_does() {
    for (kind, trap) in kinds() {
        assert_eq!(kind.to_string(), trap.to_string(), "{kind:?}");
    }
}

#[wcmp_macros::test]
fn it_names_each_core_trap_as_wasmtime_does() {
    for (kind, trap) in kinds() {
        assert_eq!(format!("{kind:?}"), format!("{trap:?}"));
    }
}

#[wcmp_macros::test]
fn it_words_a_trap_as_the_error_it_is() {
    let error = Error::from(TrapKind::IntegerDivisionByZero);
    assert_eq!(error.to_string(), "wasm trap: integer divide by zero");
}

#[wcmp_macros::test]
fn it_keeps_the_message_of_the_engine_for_a_trap_it_cannot_tell() {
    let kind = TrapKind::Other("RuntimeError: something new".to_string());
    assert_eq!(kind.to_string(), "RuntimeError: something new");
}

#[wcmp_macros::test]
fn it_words_a_host_error_as_the_host_does() {
    let kind = TrapKind::Host(anyhow::anyhow!("the host gave up"));
    assert_eq!(kind.to_string(), "the host gave up");
}
