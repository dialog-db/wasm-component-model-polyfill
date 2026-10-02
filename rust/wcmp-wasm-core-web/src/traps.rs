// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The kind of a trap, read from the message the browser's engine gives it.

use wcmp_wasm_core::TrapKind;

/// What one message of an engine tells about a trap.
#[derive(Clone, Copy)]
enum Names {
    /// The message names one kind of trap.
    One(fn() -> TrapKind),
    /// The message names more than one kind, which Wasmtime tells apart, so
    /// the trap is [`TrapKind::Other`] with the engine's message.
    Several,
    /// The message of V8 for an atomic wait that it refused. V8 refuses a
    /// wait on a memory that is not shared, and a wait on a thread that may
    /// not wait, such as the main thread of a page, with one message. So
    /// the trap is [`TrapKind::AtomicWaitNonSharedMemory`] only where no
    /// memory of the store is shared.
    Wait,
}

/// The messages of V8, the engine of Chromium, for each trap, as its
/// message templates word them.
///
/// Source: `src/common/message-template.h`, the `WasmTrap` templates and
/// `StackOverflow`, and `src/runtime/runtime-wasm.cc`, which refuses an
/// atomic wait with `AtomicsOperationNotAllowed` and throws each segment
/// template, at
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/common/message-template.h>
/// and
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/runtime/runtime-wasm.cc>.
/// A trap of an instantiation has more words: see [`instantiation`].
const V8: &[(&str, Names)] = &[
    (
        "unreachable",
        Names::One(|| TrapKind::UnreachableCodeReached),
    ),
    (
        "memory access out of bounds",
        Names::One(|| TrapKind::MemoryOutOfBounds),
    ),
    (
        "operation does not support unaligned accesses",
        Names::One(|| TrapKind::HeapMisaligned),
    ),
    (
        "divide by zero",
        Names::One(|| TrapKind::IntegerDivisionByZero),
    ),
    (
        "remainder by zero",
        Names::One(|| TrapKind::IntegerDivisionByZero),
    ),
    (
        "divide result unrepresentable",
        Names::One(|| TrapKind::IntegerOverflow),
    ),
    // A truncation of a NaN, which Wasmtime calls `BadConversionToInteger`,
    // and of a float outside the range of the integer, which it calls
    // `IntegerOverflow`.
    ("float unrepresentable in integer range", Names::Several),
    (
        "table index is out of bounds",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    ("null function", Names::One(|| TrapKind::IndirectCallToNull)),
    (
        "function signature mismatch",
        Names::One(|| TrapKind::BadSignature),
    ),
    (
        "dereferencing a null pointer",
        Names::One(|| TrapKind::NullReference),
    ),
    ("illegal cast", Names::One(|| TrapKind::CastFailure)),
    (
        "array element access out of bounds",
        Names::One(|| TrapKind::ArrayOutOfBounds),
    ),
    (
        "requested new array is too large",
        Names::One(|| TrapKind::AllocationTooLarge),
    ),
    // A range of a data segment out of its bounds, in `array.new_data` and
    // `array.init_data`, which the specification and Wasmtime call an
    // access out of the bounds of a memory. `memory.init` takes the
    // message of a memory.
    (
        "data segment out of bounds",
        Names::One(|| TrapKind::MemoryOutOfBounds),
    ),
    // A range of an element segment out of its bounds, in `table.init`,
    // `array.new_elem`, and `array.init_elem`, which the specification and
    // Wasmtime call an access out of the bounds of a table.
    (
        "element segment out of bounds",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    (
        "WasmFX: unhandled suspend",
        Names::One(|| TrapKind::UnhandledTag),
    ),
    ("Atomics.wait cannot be called in this context", Names::Wait),
    (
        "Maximum call stack size exceeded",
        Names::One(|| TrapKind::StackOverflow),
    ),
];

/// The messages of SpiderMonkey, the engine of Firefox, for each trap.
///
/// Source: `js/public/friend/ErrorNumbers.msg`, the `JSMSG_WASM` messages
/// and `JSMSG_OVER_RECURSED`, with the trap each reports in
/// `js/src/wasm/WasmBuiltins.cpp` and `js/src/wasm/WasmInstance.cpp`, at
/// <https://github.com/mozilla-firefox/firefox/blob/ab8d6c5ccc0df56b48c3416e2e6f6e596fd2e6f2/js/public/friend/ErrorNumbers.msg>,
/// <https://github.com/mozilla-firefox/firefox/blob/ab8d6c5ccc0df56b48c3416e2e6f6e596fd2e6f2/js/src/wasm/WasmBuiltins.cpp>,
/// and
/// <https://github.com/mozilla-firefox/firefox/blob/ab8d6c5ccc0df56b48c3416e2e6f6e596fd2e6f2/js/src/wasm/WasmInstance.cpp>.
const SPIDERMONKEY: &[(&str, Names)] = &[
    (
        "unreachable executed",
        Names::One(|| TrapKind::UnreachableCodeReached),
    ),
    // An access out of bounds of a memory, of an array, and of a table by
    // an indirect call or a bulk instruction.
    ("index out of bounds", Names::Several),
    (
        "table index out of bounds",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    (
        "unaligned memory access",
        Names::One(|| TrapKind::HeapMisaligned),
    ),
    (
        "integer divide by zero",
        Names::One(|| TrapKind::IntegerDivisionByZero),
    ),
    ("integer overflow", Names::One(|| TrapKind::IntegerOverflow)),
    (
        "invalid conversion to integer",
        Names::One(|| TrapKind::BadConversionToInteger),
    ),
    (
        "indirect call to null",
        Names::One(|| TrapKind::IndirectCallToNull),
    ),
    (
        "indirect call signature mismatch",
        Names::One(|| TrapKind::BadSignature),
    ),
    (
        "dereferencing null pointer",
        Names::One(|| TrapKind::NullReference),
    ),
    ("bad cast", Names::One(|| TrapKind::CastFailure)),
    (
        "atomic wait on non-shared memory",
        Names::One(|| TrapKind::AtomicWaitNonSharedMemory),
    ),
    ("too much recursion", Names::One(|| TrapKind::StackOverflow)),
];

/// The messages of JavaScriptCore, the engine of Safari, for each trap.
///
/// Source: `Source/JavaScriptCore/wasm/WasmExceptionType.h`, and
/// `Source/JavaScriptCore/runtime/ExceptionHelpers.cpp` for the stack
/// overflow, which `throwWasmToJSException` in
/// `Source/JavaScriptCore/wasm/WasmOperationsInlines.h` reports as a
/// `RangeError`, at
/// <https://github.com/WebKit/WebKit/blob/1f316fb5ae9cef5d4ea3fb52168ad10100e04729/Source/JavaScriptCore/wasm/WasmExceptionType.h>,
/// <https://github.com/WebKit/WebKit/blob/1f316fb5ae9cef5d4ea3fb52168ad10100e04729/Source/JavaScriptCore/runtime/ExceptionHelpers.cpp>,
/// and
/// <https://github.com/WebKit/WebKit/blob/1f316fb5ae9cef5d4ea3fb52168ad10100e04729/Source/JavaScriptCore/wasm/WasmOperationsInlines.h>.
const JAVASCRIPTCORE: &[(&str, Names)] = &[
    (
        "Unreachable code should not be executed",
        Names::One(|| TrapKind::UnreachableCodeReached),
    ),
    // An access out of bounds of a memory, and an atomic wait that is
    // misaligned, on a memory that is not shared, or on a thread that may
    // not wait.
    ("Out of bounds memory access", Names::Several),
    (
        "Unaligned memory access",
        Names::One(|| TrapKind::HeapMisaligned),
    ),
    (
        "Out of bounds table access",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    (
        "Out of bounds call_indirect",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    // Nothing in JavaScriptCore throws this message at the commit above:
    // an indirect call to a null entry takes the message of a signature
    // that does not match. The entry stays because its words name one kind
    // alone.
    (
        "call_indirect to a null table entry",
        Names::One(|| TrapKind::IndirectCallToNull),
    ),
    // An indirect call to a signature that does not match, which Wasmtime
    // calls `BadSignature`, and to a null entry, which it calls
    // `IndirectCallToNull`. Each tier checks a
    // null entry as a signature that does not match: the interpreter at
    // `WasmIPIntSlowPaths.cpp:1203-1204`, the baseline compiler at
    // `WasmBBQJIT.cpp:4954-4957`, and the optimizing compiler at
    // `WasmOMGIRGenerator.cpp:6825-6840`, all in
    // `Source/JavaScriptCore/wasm/` at the commit above.
    (
        "call_indirect to a signature that does not match",
        Names::Several,
    ),
    (
        "call_ref to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "i31.get_<sx> to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "access to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "ref.as_non_null to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    // A truncation of a NaN, and of a float outside the range of the
    // integer.
    ("Out of bounds Trunc operation", Names::Several),
    (
        "Division by zero",
        Names::One(|| TrapKind::IntegerDivisionByZero),
    ),
    ("Integer overflow", Names::One(|| TrapKind::IntegerOverflow)),
    (
        "Out of bounds array.get",
        Names::One(|| TrapKind::ArrayOutOfBounds),
    ),
    (
        "Out of bounds array.set",
        Names::One(|| TrapKind::ArrayOutOfBounds),
    ),
    (
        "Out of bounds array.fill",
        Names::One(|| TrapKind::ArrayOutOfBounds),
    ),
    (
        "Out of bounds array.copy",
        Names::One(|| TrapKind::ArrayOutOfBounds),
    ),
    (
        "Failed to allocate new array",
        Names::One(|| TrapKind::AllocationTooLarge),
    ),
    (
        "ref.cast failed to cast reference to target heap type",
        Names::One(|| TrapKind::CastFailure),
    ),
    (
        "Maximum call stack size exceeded.",
        Names::One(|| TrapKind::StackOverflow),
    ),
];

/// The kind of the trap whose message, in the words of the browser's
/// engine, is `message`, in a store where `shared_memory` tells whether any
/// memory is shared.
///
/// The browser gives no trap code, so the backend reads the kind from the
/// message, through the table of each engine above. The backend does not
/// know which engine it runs on, and no message of one engine means
/// another kind in another engine, so it reads all three tables. A message
/// that names more than one kind, or that no table knows, is
/// [`TrapKind::Other`] with the engine's message, never a wrong kind.
///
/// JavaScriptCore can add the source of a call to a message, as in
/// `Division by zero (evaluating 'f()')`. The table knows the message
/// without it. V8 puts the name of the call of the JavaScript API before
/// the message of a trap that an instantiation raised: see
/// [`instantiation`].
pub fn kind(message: &str, shared_memory: bool) -> TrapKind {
    let other = || TrapKind::Other(message.to_string());
    match names(message) {
        Some(Names::One(kind)) => kind(),
        Some(Names::Wait) if !shared_memory => TrapKind::AtomicWaitNonSharedMemory,
        Some(Names::Wait | Names::Several) | None => other(),
    }
}

/// What the tables tell about the message `message`, where one knows it.
fn names(message: &str) -> Option<Names> {
    let message = message
        .find(" (evaluating '")
        .filter(|_| message.ends_with("')"))
        .map_or(message, |end| &message[..end]);
    let message = match instantiation(message) {
        Instantiation::Trap(message) => message,
        Instantiation::DataSegment => return Some(Names::One(|| TrapKind::MemoryOutOfBounds)),
    };
    [V8, SPIDERMONKEY, JAVASCRIPTCORE]
        .into_iter()
        .flatten()
        .find(|(known, _)| *known == message)
        .map(|&(_, names)| names)
}

/// What a message of V8 tells, once the name of its call is taken off.
enum Instantiation<'a> {
    /// A message whose words the tables know, or do not know.
    Trap(&'a str),
    /// A data segment out of the bounds of its memory, which the
    /// specification and Wasmtime call an access out of the bounds of a
    /// memory.
    DataSegment,
}

/// The message `message` of a trap, without the name of the call of the
/// JavaScript API that V8 puts before it, as in
/// `WebAssembly.instantiate(): table index is out of bounds`.
///
/// V8 raises a trap of an instantiation through its `ErrorThrower`, which
/// words it as the name of the call, `: `, and the message: the template of
/// the trap, or the words of a data segment out of bounds, as in
/// `data segment 0 is out of bounds (offset 65536, length 1, memory size
/// 65536)`. Source: `src/wasm/wasm-result.cc` and
/// `src/wasm/module-instantiate.cc`, at
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/wasm/wasm-result.cc#L58-L69>
/// and
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/wasm/module-instantiate.cc#L1507-L1513>.
fn instantiation(message: &str) -> Instantiation<'_> {
    let Some(trap) = message
        .strip_prefix("WebAssembly.")
        .and_then(|rest| rest.split_once("(): "))
        .filter(|(call, _)| call.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|(_, trap)| trap)
    else {
        return Instantiation::Trap(message);
    };
    let data_segment = trap
        .strip_prefix("data segment ")
        .and_then(|rest| rest.split_once(" is out of bounds (offset "))
        .is_some_and(|(index, _)| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()));
    if data_segment {
        Instantiation::DataSegment
    } else {
        Instantiation::Trap(trap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message of the tables, with what it names.
    fn every_message() -> impl Iterator<Item = &'static (&'static str, Names)> {
        [V8, SPIDERMONKEY, JAVASCRIPTCORE].into_iter().flatten()
    }

    #[wcmp_macros::test]
    fn it_knows_each_message_in_one_table_alone() {
        let mut seen = std::collections::HashSet::new();
        for (message, _) in every_message() {
            assert!(seen.insert(*message), "{message:?} is in two tables");
        }
    }

    #[wcmp_macros::test]
    fn it_gives_each_kind_the_message_of_wasmtime() {
        for (message, names) in every_message() {
            if let Names::One(_) = names {
                let kind = kind(message, false);
                assert!(
                    kind.to_string().starts_with("wasm trap: "),
                    "{message:?} reads as {kind}"
                );
            }
        }
    }

    /// What a test expects of one message of an engine.
    enum Expected {
        /// The kind of trap, as its `Debug` form names it.
        Kind(&'static str),
        /// More than one kind, so [`TrapKind::Other`] with the message.
        Several,
        /// A refused wait, as [`Names::Wait`] reads it.
        Wait,
    }

    /// Asserts that `table` holds exactly the messages of `pinned`, in its
    /// order, and that each reads as `pinned` expects.
    ///
    /// The expectations are written out here, apart from the table, so a
    /// change to a table entry fails until the engine's source is looked
    /// at again.
    fn assert_pinned(table: &[(&str, Names)], pinned: &[(&str, Expected)]) {
        assert_eq!(
            table
                .iter()
                .map(|(message, _)| *message)
                .collect::<Vec<_>>(),
            pinned
                .iter()
                .map(|(message, _)| *message)
                .collect::<Vec<_>>(),
            "the table and its pinned messages differ"
        );
        for (message, expected) in pinned {
            let read = kind(message, false);
            match expected {
                Expected::Kind(name) => {
                    assert_eq!(format!("{read:?}"), *name, "{message:?}");
                }
                Expected::Several => assert!(
                    matches!(&read, TrapKind::Other(text) if text == message),
                    "{message:?} reads as {read:?}"
                ),
                Expected::Wait => {
                    assert!(
                        matches!(read, TrapKind::AtomicWaitNonSharedMemory),
                        "{message:?} reads as {read:?}"
                    );
                    let shared = kind(message, true);
                    assert!(
                        matches!(&shared, TrapKind::Other(text) if text == message),
                        "{message:?} reads as {shared:?} where a memory is shared"
                    );
                }
            }
        }
    }

    #[wcmp_macros::test]
    fn it_reads_each_message_of_v8_as_its_pinned_kind() {
        use Expected::*;
        assert_pinned(
            V8,
            &[
                ("unreachable", Kind("UnreachableCodeReached")),
                ("memory access out of bounds", Kind("MemoryOutOfBounds")),
                (
                    "operation does not support unaligned accesses",
                    Kind("HeapMisaligned"),
                ),
                ("divide by zero", Kind("IntegerDivisionByZero")),
                ("remainder by zero", Kind("IntegerDivisionByZero")),
                ("divide result unrepresentable", Kind("IntegerOverflow")),
                ("float unrepresentable in integer range", Several),
                ("table index is out of bounds", Kind("TableOutOfBounds")),
                ("null function", Kind("IndirectCallToNull")),
                ("function signature mismatch", Kind("BadSignature")),
                ("dereferencing a null pointer", Kind("NullReference")),
                ("illegal cast", Kind("CastFailure")),
                (
                    "array element access out of bounds",
                    Kind("ArrayOutOfBounds"),
                ),
                (
                    "requested new array is too large",
                    Kind("AllocationTooLarge"),
                ),
                ("data segment out of bounds", Kind("MemoryOutOfBounds")),
                ("element segment out of bounds", Kind("TableOutOfBounds")),
                ("WasmFX: unhandled suspend", Kind("UnhandledTag")),
                ("Atomics.wait cannot be called in this context", Wait),
                ("Maximum call stack size exceeded", Kind("StackOverflow")),
            ],
        );
    }

    #[wcmp_macros::test]
    fn it_reads_each_message_of_spidermonkey_as_its_pinned_kind() {
        use Expected::*;
        assert_pinned(
            SPIDERMONKEY,
            &[
                ("unreachable executed", Kind("UnreachableCodeReached")),
                ("index out of bounds", Several),
                ("table index out of bounds", Kind("TableOutOfBounds")),
                ("unaligned memory access", Kind("HeapMisaligned")),
                ("integer divide by zero", Kind("IntegerDivisionByZero")),
                ("integer overflow", Kind("IntegerOverflow")),
                (
                    "invalid conversion to integer",
                    Kind("BadConversionToInteger"),
                ),
                ("indirect call to null", Kind("IndirectCallToNull")),
                ("indirect call signature mismatch", Kind("BadSignature")),
                ("dereferencing null pointer", Kind("NullReference")),
                ("bad cast", Kind("CastFailure")),
                (
                    "atomic wait on non-shared memory",
                    Kind("AtomicWaitNonSharedMemory"),
                ),
                ("too much recursion", Kind("StackOverflow")),
            ],
        );
    }

    #[wcmp_macros::test]
    fn it_reads_each_message_of_javascriptcore_as_its_pinned_kind() {
        use Expected::*;
        assert_pinned(
            JAVASCRIPTCORE,
            &[
                (
                    "Unreachable code should not be executed",
                    Kind("UnreachableCodeReached"),
                ),
                ("Out of bounds memory access", Several),
                ("Unaligned memory access", Kind("HeapMisaligned")),
                ("Out of bounds table access", Kind("TableOutOfBounds")),
                ("Out of bounds call_indirect", Kind("TableOutOfBounds")),
                (
                    "call_indirect to a null table entry",
                    Kind("IndirectCallToNull"),
                ),
                ("call_indirect to a signature that does not match", Several),
                ("call_ref to a null reference", Kind("NullReference")),
                ("i31.get_<sx> to a null reference", Kind("NullReference")),
                ("access to a null reference", Kind("NullReference")),
                ("ref.as_non_null to a null reference", Kind("NullReference")),
                ("Out of bounds Trunc operation", Several),
                ("Division by zero", Kind("IntegerDivisionByZero")),
                ("Integer overflow", Kind("IntegerOverflow")),
                ("Out of bounds array.get", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.set", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.fill", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.copy", Kind("ArrayOutOfBounds")),
                ("Failed to allocate new array", Kind("AllocationTooLarge")),
                (
                    "ref.cast failed to cast reference to target heap type",
                    Kind("CastFailure"),
                ),
                ("Maximum call stack size exceeded.", Kind("StackOverflow")),
            ],
        );
    }

    #[wcmp_macros::test]
    fn it_reads_a_message_that_javascriptcore_gave_a_source() {
        assert!(matches!(
            kind("Division by zero (evaluating 'f()')", false),
            TrapKind::IntegerDivisionByZero
        ));
    }

    #[wcmp_macros::test]
    fn it_reads_a_trap_of_an_instantiation_without_the_name_of_its_call() {
        for (message, expected) in [
            (
                "WebAssembly.instantiate(): table index is out of bounds",
                "TableOutOfBounds",
            ),
            (
                "WebAssembly.Instance(): data segment 0 is out of bounds \
                 (offset 65536, length 1, memory size 65536)",
                "MemoryOutOfBounds",
            ),
            (
                "WebAssembly.instantiate(): data segment 12 is out of bounds \
                 (offset 4294967295, length 1, memory size 0)",
                "MemoryOutOfBounds",
            ),
        ] {
            assert_eq!(
                format!("{:?}", kind(message, false)),
                expected,
                "{message:?}"
            );
        }
        for message in [
            "WebAssembly.instantiate(): float unrepresentable in integer range",
            "WebAssembly.instantiate(): data segment is out of bounds (offset 1)",
            "data segment 0 is out of bounds (offset 1, length 1, memory size 0)",
        ] {
            assert!(
                matches!(kind(message, false), TrapKind::Other(ref text) if text == message),
                "{message:?}"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_leaves_a_message_it_does_not_know_as_other() {
        let kind = kind("a trap no engine words this way", false);
        assert!(
            matches!(&kind, TrapKind::Other(message) if message == "a trap no engine words this way")
        );
        assert_eq!(kind.to_string(), "a trap no engine words this way");
    }

    #[wcmp_macros::test]
    fn it_leaves_a_message_of_more_than_one_kind_as_other() {
        for (message, names) in every_message() {
            if let Names::Several = names {
                assert!(
                    matches!(kind(message, false), TrapKind::Other(ref text) if text == message),
                    "{message:?}"
                );
            }
        }
    }

    #[wcmp_macros::test]
    fn it_reads_a_refused_wait_as_non_shared_only_where_no_memory_is_shared() {
        let message = "Atomics.wait cannot be called in this context";
        assert!(matches!(
            kind(message, false),
            TrapKind::AtomicWaitNonSharedMemory
        ));
        assert!(matches!(kind(message, true), TrapKind::Other(ref text) if text == message));
    }
}
