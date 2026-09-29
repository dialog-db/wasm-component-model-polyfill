//! Wasmi's errors, as the runtime layer reports them.

use core::fmt::Display;

use wasmi::errors::{ErrorKind, InstantiationError};
use wcmp_wasm_core::{Error, TrapKind};

use crate::host_error::HostError;
use crate::suspension::Suspension;

/// [`Error::Backend`], with Wasmi's words for `error`.
pub fn backend(error: impl Display) -> Error {
    Error::Backend {
        message: error.to_string(),
    }
}

/// The trap that `error`, which a call into a guest returned, stands for.
pub fn trap(error: wasmi::Error) -> Error {
    Error::Trap(trap_kind(error))
}

/// The kind of the trap that `error` stands for.
///
/// A host function's own error comes back as [`TrapKind::Host`], unchanged.
/// The marker of a suspending host function that answered "not yet" where
/// its call cannot suspend comes back as [`TrapKind::Host`] too, with the
/// marker's message, as every backend reports it. A trap of Wasmi's with a
/// trap code comes back as the kind of the same name, and anything else as
/// [`TrapKind::Other`] with Wasmi's message.
fn trap_kind(error: wasmi::Error) -> TrapKind {
    if let Some(suspension) = error.downcast_ref::<Suspension>() {
        return TrapKind::Host(anyhow::anyhow!("{suspension}"));
    }
    // `downcast` takes the error, so the other readings of it come first.
    let core = error.as_trap_code().and_then(core_trap);
    let message = error.to_string();
    match error.downcast::<HostError>() {
        Some(HostError(error)) => TrapKind::Host(error),
        None => core.unwrap_or(TrapKind::Other(message)),
    }
}

/// The kind of the trap `code`, or `None` for a trap code that names no
/// core trap of the runtime layer.
///
/// Each kind carries Wasmtime's message for the trap, and never Wasmi's
/// own words. The match lists every trap code of Wasmi's, so a trap code
/// that a later Wasmi adds does not build until it has a place here.
/// `OutOfFuel` maps to the kind the runtime layer reserves for it: the
/// backend does not turn fuel on, so Wasmi does not raise it today. A trap
/// of a limit or of the host's memory is not a trap of WebAssembly, and it
/// has no kind of its own.
pub fn core_trap(code: wasmi::TrapCode) -> Option<TrapKind> {
    Some(match code {
        wasmi::TrapCode::UnreachableCodeReached => TrapKind::UnreachableCodeReached,
        wasmi::TrapCode::MemoryOutOfBounds => TrapKind::MemoryOutOfBounds,
        wasmi::TrapCode::TableOutOfBounds => TrapKind::TableOutOfBounds,
        wasmi::TrapCode::IndirectCallToNull => TrapKind::IndirectCallToNull,
        wasmi::TrapCode::IntegerDivisionByZero => TrapKind::IntegerDivisionByZero,
        wasmi::TrapCode::IntegerOverflow => TrapKind::IntegerOverflow,
        wasmi::TrapCode::BadConversionToInteger => TrapKind::BadConversionToInteger,
        wasmi::TrapCode::StackOverflow => TrapKind::StackOverflow,
        wasmi::TrapCode::BadSignature => TrapKind::BadSignature,
        wasmi::TrapCode::OutOfFuel => TrapKind::OutOfFuel,
        wasmi::TrapCode::GrowthOperationLimited | wasmi::TrapCode::OutOfSystemMemory => {
            return None;
        }
    })
}

/// The error of an instantiation that failed with `error`.
///
/// Wasmi checks every import before it initializes the instance. An import
/// that does not fit is [`Error::Link`], named by the import Wasmi reports.
/// A segment that does not fit its table or its memory is the trap the
/// specification makes of it, as is a failure of the start function.
pub fn instantiation(error: wasmi::Error) -> Error {
    let name = match error.kind() {
        ErrorKind::Instantiation(
            InstantiationError::ImportTypeMismatch { name, .. }
            | InstantiationError::GlobalTypeMismatch { name, .. }
            | InstantiationError::FuncTypeMismatch { name, .. }
            | InstantiationError::TableTypeMismatch { name, .. }
            | InstantiationError::MemoryTypeMismatch { name, .. },
        ) => name,
        ErrorKind::Instantiation(InstantiationError::MismatchedNumberOfImports {
            expected,
            actual,
        }) => {
            return Error::ImportCount {
                expected: *expected,
                actual: *actual,
            };
        }
        ErrorKind::Instantiation(InstantiationError::ElementSegmentDoesNotFit { .. }) => {
            return Error::Trap(TrapKind::TableOutOfBounds);
        }
        ErrorKind::Instantiation(_) => return backend(error),
        _ if error.downcast_ref::<HostError>().is_some()
            || error.downcast_ref::<Suspension>().is_some()
            || error.as_trap_code().is_some() =>
        {
            return trap(error);
        }
        _ => return backend(error),
    };
    Error::Link {
        module: name.module().to_string(),
        name: name.name().to_string(),
        message: error.to_string(),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// Every trap code of Wasmi's that names a core trap.
    const CORE_TRAPS: [wasmi::TrapCode; 10] = [
        wasmi::TrapCode::UnreachableCodeReached,
        wasmi::TrapCode::MemoryOutOfBounds,
        wasmi::TrapCode::TableOutOfBounds,
        wasmi::TrapCode::IndirectCallToNull,
        wasmi::TrapCode::IntegerDivisionByZero,
        wasmi::TrapCode::IntegerOverflow,
        wasmi::TrapCode::BadConversionToInteger,
        wasmi::TrapCode::StackOverflow,
        wasmi::TrapCode::BadSignature,
        wasmi::TrapCode::OutOfFuel,
    ];

    #[wcmp_macros::test]
    fn it_maps_each_core_trap_to_the_kind_of_its_name() {
        for code in CORE_TRAPS {
            let kind = core_trap(code).unwrap_or_else(|| panic!("{code:?} is a core trap"));
            assert_eq!(format!("{kind:?}"), format!("{code:?}"));
        }
    }

    #[wcmp_macros::test]
    fn it_gives_each_kind_wasmtimes_message_and_not_wasmis() {
        let kind = core_trap(wasmi::TrapCode::IndirectCallToNull).expect("a core trap");
        assert_eq!(kind.to_string(), "wasm trap: uninitialized element");
        assert_ne!(
            kind.to_string(),
            wasmi::TrapCode::IndirectCallToNull.trap_message()
        );
    }

    #[wcmp_macros::test]
    fn it_leaves_a_trap_of_a_limit_to_other() {
        for code in [
            wasmi::TrapCode::GrowthOperationLimited,
            wasmi::TrapCode::OutOfSystemMemory,
        ] {
            assert!(core_trap(code).is_none(), "{code:?}");
            let kind = trap_kind(wasmi::Error::from(code));
            assert!(
                matches!(&kind, TrapKind::Other(message) if message == code.trap_message()),
                "{kind:?}"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_reports_a_suspension_that_cannot_suspend_as_a_host_trap() {
        let kind = trap_kind(wasmi::Error::host(Suspension));
        assert!(
            matches!(&kind, TrapKind::Host(error) if error.to_string() == Suspension.to_string()),
            "{kind:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_gives_a_host_error_back_unchanged() {
        let error = wasmi::Error::host(HostError(anyhow::anyhow!("the host failed")));
        let kind = trap_kind(error);
        assert!(
            matches!(&kind, TrapKind::Host(error) if error.to_string() == "the host failed"),
            "{kind:?}"
        );
    }
}
