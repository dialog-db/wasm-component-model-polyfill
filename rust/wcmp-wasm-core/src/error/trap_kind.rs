// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structured kind of a trap.

use thiserror::Error;

use crate::values::ExnRef;

/// The kind of a trap, the same on every backend.
///
/// The names of the core traps, and the message of each, are Wasmtime's.
/// A backend whose engine words a trap in its own way maps it to one of
/// these kinds, and the host reads one message for one trap on every
/// backend. Where a backend cannot tell the kind, it reports
/// [`TrapKind::Other`] with the message of its engine, never a wrong kind.
///
/// The traps Wasmtime has for the Component Model are not here. The
/// polyfill owns them.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrapKind {
    /// Code that was supposed to be unreachable was reached.
    #[error("wasm trap: wasm `unreachable` instruction executed")]
    UnreachableCodeReached,

    /// A memory access fell outside its memory.
    #[error("wasm trap: out of bounds memory access")]
    MemoryOutOfBounds,

    /// A table access fell outside its table.
    #[error("wasm trap: undefined element: out of bounds table access")]
    TableOutOfBounds,

    /// An indirect call reached a null table element.
    #[error("wasm trap: uninitialized element")]
    IndirectCallToNull,

    /// The signature of an indirect call did not match its callee.
    #[error("wasm trap: indirect call type mismatch")]
    BadSignature,

    /// An integer operation overflowed.
    #[error("wasm trap: integer overflow")]
    IntegerOverflow,

    /// An integer was divided by zero.
    #[error("wasm trap: integer divide by zero")]
    IntegerDivisionByZero,

    /// A conversion from a float to an integer failed.
    #[error("wasm trap: invalid conversion to integer")]
    BadConversionToInteger,

    /// The stack was exhausted.
    #[error("wasm trap: call stack exhausted")]
    StackOverflow,

    /// A null reference was used where a reference was needed.
    #[error("wasm trap: null reference")]
    NullReference,

    /// An array access fell outside its array.
    #[error("wasm trap: out of bounds array access")]
    ArrayOutOfBounds,

    /// An allocation was too large to succeed.
    #[error("wasm trap: allocation size too large")]
    AllocationTooLarge,

    /// A reference was cast to a type it is not an instance of.
    #[error("wasm trap: cast failure")]
    CastFailure,

    /// A suspension reached a tag that no handler handles.
    #[error("wasm trap: unhandled tag")]
    UnhandledTag,

    /// A continuation was resumed a second time.
    #[error("wasm trap: continuation already consumed")]
    ContinuationAlreadyConsumed,

    /// An atomic operation was given an address that is not naturally
    /// aligned.
    #[error("wasm trap: unaligned atomic")]
    HeapMisaligned,

    /// An atomic wait was made on a memory that is not shared.
    #[error("wasm trap: atomic wait on non-shared memory")]
    AtomicWaitNonSharedMemory,

    /// Reserved: the guest consumed all of its fuel. No backend raises it.
    #[error("wasm trap: all fuel consumed by WebAssembly")]
    OutOfFuel,

    /// Reserved: the guest was interrupted. No backend raises it.
    #[error("wasm trap: interrupt")]
    Interrupt,

    /// A host function returned an error. The error is the host's own,
    /// unchanged. No guest can catch this trap.
    ///
    /// A host function that gave a result of the wrong type traps this way
    /// too: the error is then the runtime layer's own
    /// [`Error::TypeMismatch`](crate::Error::TypeMismatch), and not a kind
    /// of its own, because the fault is the host's and not the guest's.
    #[error(transparent)]
    Host(anyhow::Error),

    /// An exception reached the host with nothing in the guest to catch it.
    /// The exception is an opaque reference that the host can give back to
    /// a guest of the same store.
    #[error("thrown Wasm exception")]
    UncaughtException(ExnRef),

    /// A trap whose kind the backend cannot tell. The message is the
    /// engine's.
    #[error("{0}")]
    Other(String),
}
