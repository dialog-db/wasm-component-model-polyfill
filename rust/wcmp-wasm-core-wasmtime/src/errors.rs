//! Wasmtime's errors, as the runtime layer reports them.

use wasmtime::AsContextMut;
use wcmp_wasm_core::{Error, ImportType, TrapKind};

use crate::host_error::HostError;
use crate::state::State;

/// [`Error::Backend`], with Wasmtime's words for `error`.
pub fn backend(error: wasmtime::Error) -> Error {
    Error::Backend {
        message: format!("{error:#}"),
    }
}

/// [`Error::Backend`] for a continuation reference that would cross
/// between the host and a guest, which Wasmtime's embedding API cannot
/// carry yet.
pub fn continuation() -> Error {
    Error::Backend {
        message: "Wasmtime's embedding API does not yet carry a continuation reference \
                  between the host and a guest (bytecodealliance/wasmtime#10248)"
            .to_string(),
    }
}

/// The trap that `error`, which a call into a guest of `store` returned,
/// stands for.
pub fn trap(store: &mut impl AsContextMut<Data = State>, error: wasmtime::Error) -> Error {
    Error::Trap(trap_kind(store, error))
}

/// The kind of the trap that `error` stands for.
///
/// A host function's own error comes back as [`TrapKind::Host`], unchanged.
/// An exception that no guest caught comes back as
/// [`TrapKind::UncaughtException`], with the exception rooted in the store.
/// A core trap of Wasmtime's comes back as the kind of the same name, and
/// anything else as [`TrapKind::Other`] with Wasmtime's message.
fn trap_kind(store: &mut impl AsContextMut<Data = State>, error: wasmtime::Error) -> TrapKind {
    let error = match error.downcast::<HostError>() {
        Ok(HostError(error)) => return TrapKind::Host(error),
        Err(error) => error,
    };
    if error.is::<wasmtime::ThrownException>()
        && let Some(exception) = store.as_context_mut().take_pending_exception()
    {
        return match exception.to_owned_rooted(&mut *store) {
            Ok(exception) => TrapKind::UncaughtException(
                store.as_context_mut().data_mut().add_exn_ref(exception),
            ),
            Err(error) => TrapKind::Other(format!("{error:#}")),
        };
    }
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>()
        && let Some(kind) = core_trap(*trap)
    {
        return kind;
    }
    TrapKind::Other(error.root_cause().to_string())
}

/// The kind of the core trap `trap`, or `None` for a trap that is not a
/// core trap, such as one of the Component Model's.
fn core_trap(trap: wasmtime::Trap) -> Option<TrapKind> {
    Some(match trap {
        wasmtime::Trap::StackOverflow => TrapKind::StackOverflow,
        wasmtime::Trap::MemoryOutOfBounds => TrapKind::MemoryOutOfBounds,
        wasmtime::Trap::HeapMisaligned => TrapKind::HeapMisaligned,
        wasmtime::Trap::TableOutOfBounds => TrapKind::TableOutOfBounds,
        wasmtime::Trap::IndirectCallToNull => TrapKind::IndirectCallToNull,
        wasmtime::Trap::BadSignature => TrapKind::BadSignature,
        wasmtime::Trap::IntegerOverflow => TrapKind::IntegerOverflow,
        wasmtime::Trap::IntegerDivisionByZero => TrapKind::IntegerDivisionByZero,
        wasmtime::Trap::BadConversionToInteger => TrapKind::BadConversionToInteger,
        wasmtime::Trap::UnreachableCodeReached => TrapKind::UnreachableCodeReached,
        wasmtime::Trap::AtomicWaitNonSharedMemory => TrapKind::AtomicWaitNonSharedMemory,
        wasmtime::Trap::NullReference => TrapKind::NullReference,
        wasmtime::Trap::ArrayOutOfBounds => TrapKind::ArrayOutOfBounds,
        wasmtime::Trap::AllocationTooLarge => TrapKind::AllocationTooLarge,
        wasmtime::Trap::CastFailure => TrapKind::CastFailure,
        wasmtime::Trap::UnhandledTag => TrapKind::UnhandledTag,
        wasmtime::Trap::ContinuationAlreadyConsumed => TrapKind::ContinuationAlreadyConsumed,
        _ => return None,
    })
}

/// The error of an instantiation of a module with `imports` that failed
/// with `error`.
///
/// Wasmtime checks every import before it runs the start function. An
/// import that does not fit is [`Error::Link`], named by the context
/// Wasmtime gives the error. A failure of the start function is a trap.
pub fn instantiation(
    store: &mut impl AsContextMut<Data = State>,
    imports: &[ImportType],
    error: wasmtime::Error,
) -> Error {
    if error.is::<HostError>()
        || error.is::<wasmtime::ThrownException>()
        || error.is::<wasmtime::Trap>()
    {
        return trap(store, error);
    }
    let context = error.to_string();
    let import = imports.iter().find(|import| {
        context
            == format!(
                "incompatible import type for `{}::{}`",
                import.module(),
                import.name()
            )
    });
    match import {
        Some(import) => Error::Link {
            module: import.module().to_string(),
            name: import.name().to_string(),
            message: error
                .chain()
                .skip(1)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(": "),
        },
        None => backend(error),
    }
}
