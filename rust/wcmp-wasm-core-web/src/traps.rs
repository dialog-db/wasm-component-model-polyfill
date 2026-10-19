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
    /// The message of JavaScriptCore for an access out of the bounds of a
    /// memory, which it gives an atomic wait or notify that it refused, or
    /// that is misaligned, too. So the trap is
    /// [`TrapKind::MemoryOutOfBounds`] only where no module of the store
    /// waits or notifies.
    Memory,
}

/// What the modules of a store tell about a trap whose message names more
/// than one kind in some engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct Atomics {
    /// Whether a memory of an instance of the store is shared.
    pub shared_memory: bool,
    /// Whether a module of the store waits or notifies on a memory.
    pub atomic_wait: bool,
}

impl Atomics {
    /// What a step knows that cannot tell which store it ran in: that
    /// any memory may be shared, and any module may wait.
    pub const UNKNOWN: Self = Self {
        shared_memory: true,
        atomic_wait: true,
    };
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
    // A `resume` of a continuation that was consumed, which Wasmtime
    // calls `ContinuationAlreadyConsumed`, and of a null continuation,
    // which it calls `NullReference`.
    ("WasmFX: resuming an invalid continuation", Names::Several),
    // A `throw_ref` of a null reference.
    (
        "rethrowing null value",
        Names::One(|| TrapKind::NullReference),
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
    // An `array.new` of more elements than the engine allows.
    (
        "too many array elements",
        Names::One(|| TrapKind::AllocationTooLarge),
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
/// WebKitGTK 2.54, which the WebKitGTK lanes run, has two messages more,
/// for `array.fill` and `array.copy` on a null reference, at
/// <https://github.com/WebKit/WebKit/blob/628d6ce1850e17225185a5807a9d0aa375bae2be/Source/JavaScriptCore/wasm/WasmExceptionType.h#L64-L65>.
/// A trap of an instantiation has words of its own: see
/// [`javascriptcore_instantiation`].
const JAVASCRIPTCORE: &[(&str, Names)] = &[
    (
        "Unreachable code should not be executed",
        Names::One(|| TrapKind::UnreachableCodeReached),
    ),
    // An access out of bounds of a memory, and an atomic wait or notify
    // that is misaligned, a wait on a memory that is not shared, or a wait
    // on a thread that may not wait.
    ("Out of bounds memory access", Names::Memory),
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
        "throw_ref on a null reference",
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
        "array.fill to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "array.copy to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "array.init_elem to a null reference",
        Names::One(|| TrapKind::NullReference),
    ),
    (
        "array.init_data to a null reference",
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
    // The four messages of an array made or filled from a segment name
    // more than one kind: the operations of
    // `Source/JavaScriptCore/wasm/WasmOperationsInlines.h` answer one
    // failure for each range they check, as `arrayInitData` does at lines
    // 433-460 at the commit above, and each tier throws one message for
    // it, as `WasmBBQJIT.cpp` does at lines 1664, 1671, 1805, and 1844.
    //
    // A range out of the bounds of the element segment, which Wasmtime
    // calls `TableOutOfBounds`, and of the array, which it calls
    // `ArrayOutOfBounds`.
    ("Out of bounds array.init_elem", Names::Several),
    // A range out of the bounds of the data segment, which Wasmtime calls
    // `MemoryOutOfBounds`, and of the array, which it calls
    // `ArrayOutOfBounds`.
    ("Out of bounds array.init_data", Names::Several),
    // A range out of the bounds of the element segment, which Wasmtime
    // calls `TableOutOfBounds`, and an array too large to allocate, which
    // it calls `AllocationTooLarge`.
    (
        "Out of bounds or failed to allocate in array.new_elem",
        Names::Several,
    ),
    // A range out of the bounds of the data segment, which Wasmtime calls
    // `MemoryOutOfBounds`, and an array too large to allocate.
    (
        "Out of bounds or failed to allocate in array.new_data",
        Names::Several,
    ),
    // The two messages of a segment's range below are declared in the
    // header, but nothing in JavaScriptCore throws them at the commit
    // above, as for a null table entry. They stay because their words name
    // one kind alone.
    //
    // A range of a data segment out of its bounds, which the specification
    // and Wasmtime call an access out of the bounds of a memory, as V8's
    // `data segment out of bounds` above.
    (
        "Offset + array length would exceed the size of a data segment",
        Names::One(|| TrapKind::MemoryOutOfBounds),
    ),
    // A range of an element segment out of its bounds, which the
    // specification and Wasmtime call an access out of the bounds of a
    // table.
    (
        "Offset + array length would exceed the length of an element segment",
        Names::One(|| TrapKind::TableOutOfBounds),
    ),
    // An active element segment out of the bounds of its table, which an
    // instantiation raises.
    (
        "Element is trying to set an out of bounds table index",
        Names::One(|| TrapKind::TableOutOfBounds),
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
/// engine, is `message`, in a store whose modules tell `atomics`.
///
/// The browser gives no trap code, so the backend reads the kind from the
/// message, through the table of each engine above. The backend does not
/// know which engine it runs on, and no message of one engine means
/// another kind in another engine, so it reads all three tables. A message
/// that names more than one kind, or that no table knows, is
/// [`TrapKind::Other`] with the engine's message, never a wrong kind.
///
/// JavaScriptCore can add the source of a call to a message, as in
/// `Division by zero (evaluating 'f()')`. The source names the backend's
/// own call into the guest, not the guest, so the table knows the message
/// without it and [`TrapKind::Other`] carries the message without it. V8
/// puts the name of the call of the JavaScript API before the message of
/// a trap that an instantiation raised: see [`instantiation`].
pub fn kind(message: &str, atomics: Atomics) -> TrapKind {
    let message = without_source(message);
    let other = || TrapKind::Other(message.to_string());
    match names(message) {
        Some(Names::One(kind)) => kind(),
        Some(Names::Wait) if !atomics.shared_memory => TrapKind::AtomicWaitNonSharedMemory,
        Some(Names::Memory) if !atomics.atomic_wait => TrapKind::MemoryOutOfBounds,
        Some(Names::Wait | Names::Memory | Names::Several) | None => other(),
    }
}

/// The message `message` without the source of a call that
/// JavaScriptCore adds to it, as in `Division by zero (evaluating 'f()')`.
fn without_source(message: &str) -> &str {
    message
        .find(" (evaluating '")
        .filter(|_| message.ends_with("')"))
        .map_or(message, |end| &message[..end])
}

/// What the tables tell about the message `message`, where one knows it.
fn names(message: &str) -> Option<Names> {
    if javascriptcore_instantiation(message) {
        return Some(Names::One(|| TrapKind::MemoryOutOfBounds));
    }
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

/// Whether `message` is JavaScriptCore's for a data segment out of the
/// bounds of its memory, which an instantiation raises and which the
/// specification and Wasmtime call an access out of the bounds of a
/// memory.
///
/// JavaScriptCore words it with the sizes and the offset, as in `Invalid
/// data segment initialization: segment of 1 bytes memory of 65536 bytes,
/// at offset 65536, segment writes outside of memory`, and ends it with
/// `segment is too big` or `segment writes outside of memory`. Source:
/// `dataSegmentFail` in `Source/JavaScriptCore/wasm/js/WebAssemblyModuleRecord.cpp`,
/// at
/// <https://github.com/WebKit/WebKit/blob/1f316fb5ae9cef5d4ea3fb52168ad10100e04729/Source/JavaScriptCore/wasm/js/WebAssemblyModuleRecord.cpp#L847-L851>
/// and its calls at lines 961 and 965.
fn javascriptcore_instantiation(message: &str) -> bool {
    message.starts_with("Invalid data segment initialization: segment of ")
        && (message.ends_with(", segment is too big")
            || message.ends_with(", segment writes outside of memory"))
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
                let kind = kind(message, Atomics::default());
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
        /// An access out of the bounds of a memory, or a refused wait or
        /// notify, as [`Names::Memory`] reads it.
        Memory,
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
            let read = kind(message, Atomics::default());
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
                    let shared = kind(
                        message,
                        Atomics {
                            shared_memory: true,
                            ..Atomics::default()
                        },
                    );
                    assert!(
                        matches!(&shared, TrapKind::Other(text) if text == message),
                        "{message:?} reads as {shared:?} where a memory is shared"
                    );
                }
                Expected::Memory => {
                    assert!(
                        matches!(read, TrapKind::MemoryOutOfBounds),
                        "{message:?} reads as {read:?}"
                    );
                    let waits = kind(
                        message,
                        Atomics {
                            atomic_wait: true,
                            ..Atomics::default()
                        },
                    );
                    assert!(
                        matches!(&waits, TrapKind::Other(text) if text == message),
                        "{message:?} reads as {waits:?} where a module waits"
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
                ("WasmFX: resuming an invalid continuation", Several),
                ("rethrowing null value", Kind("NullReference")),
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
                ("too many array elements", Kind("AllocationTooLarge")),
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
                ("Out of bounds memory access", Memory),
                ("Unaligned memory access", Kind("HeapMisaligned")),
                ("Out of bounds table access", Kind("TableOutOfBounds")),
                ("Out of bounds call_indirect", Kind("TableOutOfBounds")),
                (
                    "call_indirect to a null table entry",
                    Kind("IndirectCallToNull"),
                ),
                ("call_indirect to a signature that does not match", Several),
                ("call_ref to a null reference", Kind("NullReference")),
                ("throw_ref on a null reference", Kind("NullReference")),
                ("i31.get_<sx> to a null reference", Kind("NullReference")),
                ("access to a null reference", Kind("NullReference")),
                ("array.fill to a null reference", Kind("NullReference")),
                ("array.copy to a null reference", Kind("NullReference")),
                ("array.init_elem to a null reference", Kind("NullReference")),
                ("array.init_data to a null reference", Kind("NullReference")),
                ("ref.as_non_null to a null reference", Kind("NullReference")),
                ("Out of bounds Trunc operation", Several),
                ("Division by zero", Kind("IntegerDivisionByZero")),
                ("Integer overflow", Kind("IntegerOverflow")),
                ("Out of bounds array.get", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.set", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.fill", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.copy", Kind("ArrayOutOfBounds")),
                ("Out of bounds array.init_elem", Several),
                ("Out of bounds array.init_data", Several),
                (
                    "Out of bounds or failed to allocate in array.new_elem",
                    Several,
                ),
                (
                    "Out of bounds or failed to allocate in array.new_data",
                    Several,
                ),
                (
                    "Offset + array length would exceed the size of a data segment",
                    Kind("MemoryOutOfBounds"),
                ),
                (
                    "Offset + array length would exceed the length of an element segment",
                    Kind("TableOutOfBounds"),
                ),
                (
                    "Element is trying to set an out of bounds table index",
                    Kind("TableOutOfBounds"),
                ),
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
            kind("Division by zero (evaluating 'f()')", Atomics::default()),
            TrapKind::IntegerDivisionByZero
        ));
    }

    #[wcmp_macros::test]
    fn it_leaves_the_source_off_a_message_of_more_than_one_kind() {
        let read = kind(
            "call_indirect to a signature that does not match \
             (evaluating 'Reflect.apply(arg0, arg1, arg2)')",
            Atomics::default(),
        );
        assert!(
            matches!(&read, TrapKind::Other(text)
                if text == "call_indirect to a signature that does not match"),
            "{read:?}"
        );
    }

    #[wcmp_macros::test]
    fn it_reads_a_data_segment_out_of_bounds_in_an_instantiation_of_javascriptcore() {
        for message in [
            "Invalid data segment initialization: segment of 1 bytes memory of 65536 bytes, \
             at offset 65536, segment writes outside of memory",
            "Invalid data segment initialization: segment of 70000 bytes memory of 65536 \
             bytes, at offset 0, segment is too big",
        ] {
            assert!(
                matches!(
                    kind(message, Atomics::default()),
                    TrapKind::MemoryOutOfBounds
                ),
                "{message:?}"
            );
        }
        let unknown = "Invalid data segment initialization: segment of 1 bytes";
        assert!(
            matches!(kind(unknown, Atomics::default()), TrapKind::Other(ref text) if text == unknown),
            "{unknown:?}"
        );
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
                format!("{:?}", kind(message, Atomics::default())),
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
                matches!(kind(message, Atomics::default()), TrapKind::Other(ref text) if text == message),
                "{message:?}"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_leaves_a_message_it_does_not_know_as_other() {
        let kind = kind("a trap no engine words this way", Atomics::default());
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
                    matches!(kind(message, Atomics::default()), TrapKind::Other(ref text) if text == message),
                    "{message:?}"
                );
            }
        }
    }

    #[wcmp_macros::test]
    fn it_reads_a_refused_wait_as_non_shared_only_where_no_memory_is_shared() {
        let message = "Atomics.wait cannot be called in this context";
        assert!(matches!(
            kind(message, Atomics::default()),
            TrapKind::AtomicWaitNonSharedMemory
        ));
        let shared = Atomics {
            shared_memory: true,
            ..Atomics::default()
        };
        assert!(matches!(kind(message, shared), TrapKind::Other(ref text) if text == message));
    }

    #[wcmp_macros::test]
    fn it_reads_an_access_out_of_a_memory_of_javascriptcore_only_where_no_module_waits() {
        let message = "Out of bounds memory access";
        assert!(matches!(
            kind(message, Atomics::default()),
            TrapKind::MemoryOutOfBounds
        ));
        // A shared memory alone does not make the message ambiguous: only
        // a wait or a notify can raise it otherwise.
        let shared = Atomics {
            shared_memory: true,
            ..Atomics::default()
        };
        assert!(matches!(kind(message, shared), TrapKind::MemoryOutOfBounds));
        let waits = Atomics {
            atomic_wait: true,
            ..Atomics::default()
        };
        assert!(matches!(kind(message, waits), TrapKind::Other(ref text) if text == message));
    }
}
