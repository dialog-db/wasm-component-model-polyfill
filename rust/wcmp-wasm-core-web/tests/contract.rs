// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The backend contract of the runtime layer, on the browser backend.
//!
//! Every test of the contract runs here. The trap fixture runs with the
//! allowances below, which differ by the engine of the page.

#![cfg(target_arch = "wasm32")]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_contract::AmbiguousTrap;
use wcmp_wasm_core_web::Web;

#[path = "support/page_engine.rs"]
mod page_engine;

use page_engine::PageEngine;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

/// The traps of the fixture that V8 words with a message of more than one
/// kind, which the backend reports as `Other` with that message.
///
/// V8 words a truncation of a NaN, which Wasmtime calls
/// `BadConversionToInteger`, and a truncation of a float outside the range
/// of the integer, which Wasmtime calls `IntegerOverflow`, with one message,
/// the template `WasmTrapFloatUnrepresentable` of
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/common/message-template.h>.
/// The test fails as stale once the backend reports `BadConversionToInteger`
/// for the trap.
///
/// V8 words a `resume` of a consumed continuation, which Wasmtime calls
/// `ContinuationAlreadyConsumed`, and of a null continuation, which
/// Wasmtime calls `NullReference`, with one message too, the template
/// `WasmTrapResume` of the same file. Chrome does not declare stack
/// switching yet, so the fixture of that trap does not run, and the
/// allowance waits for the day it does.
const AMBIGUOUS_IN_V8: &[AmbiguousTrap] = &[
    AmbiguousTrap {
        export: "bad_conversion_to_integer",
        message: "float unrepresentable in integer range",
    },
    AmbiguousTrap {
        export: "continuation_already_consumed",
        message: "WasmFX: resuming an invalid continuation",
    },
];

/// The traps of the fixture that JavaScriptCore words with a message of
/// more than one kind, which the backend reports as `Other` with that
/// message. The messages are those of
/// <https://github.com/WebKit/WebKit/blob/1f316fb5ae9cef5d4ea3fb52168ad10100e04729/Source/JavaScriptCore/wasm/WasmExceptionType.h>.
///
/// - An indirect call to a null entry, which Wasmtime calls
///   `IndirectCallToNull`, and to a function of another signature, which
///   it calls `BadSignature`: each tier of JavaScriptCore checks a null
///   entry as a signature that does not match (`WasmIPIntSlowPaths.cpp`
///   lines 1203-1204, `WasmBBQJIT.cpp` lines 4954-4957, and
///   `WasmOMGIRGenerator.cpp` lines 6825-6840, in
///   `Source/JavaScriptCore/wasm/` at the same commit), so both take the
///   message `BadSignature`, and no message tells them apart.
/// - A truncation of a NaN, which Wasmtime calls `BadConversionToInteger`,
///   and of a float outside the range of the integer, which it calls
///   `IntegerOverflow`, which JavaScriptCore words as one,
///   `OutOfBoundsTrunc`.
/// - A wait on a memory that is not shared, which Wasmtime calls
///   `AtomicWaitNonSharedMemory`, and an access out of the bounds of a
///   memory: `memoryAtomicWait32` in `WasmOperationsInlines.h` answers -1
///   for a memory that is not shared and for a thread that may not wait
///   (lines 765-768), and each tier throws `OutOfBoundsMemoryAccess`
///   where that answer is negative: the interpreter in
///   `Source/JavaScriptCore/llint/InPlaceInterpreter64.asm` (line 10122 at
///   the commit of WebKitGTK 2.54.1, 628d6ce1850e), and the baseline
///   compiler in `WasmBBQJIT.cpp` (line 1334). The backend reads the
///   message as `MemoryOutOfBounds` only in a store whose modules never
///   wait, and the fixture's module waits.
const AMBIGUOUS_IN_JAVASCRIPTCORE: &[AmbiguousTrap] = &[
    AmbiguousTrap {
        export: "indirect_call_to_null",
        message: "call_indirect to a signature that does not match",
    },
    AmbiguousTrap {
        export: "bad_signature",
        message: "call_indirect to a signature that does not match",
    },
    AmbiguousTrap {
        export: "bad_conversion_to_integer",
        message: "Out of bounds Trunc operation",
    },
    AmbiguousTrap {
        export: "atomic_wait_non_shared_memory",
        message: "Out of bounds memory access",
    },
];

#[wcmp_macros::test]
async fn it_raises_each_core_trap_the_capabilities_permit() {
    let ambiguous = match PageEngine::of_page() {
        PageEngine::V8 => AMBIGUOUS_IN_V8,
        PageEngine::JavaScriptCore => AMBIGUOUS_IN_JAVASCRIPTCORE,
        PageEngine::SpiderMonkey => &[],
    };
    wcmp_wasm_core_contract::it_raises_each_core_trap_allowing(&engine(), ambiguous).await;
}

wcmp_wasm_core_contract::contract_tests!(
    @each Web::new;
    it_calls_the_call_hook_at_each_crossing_between_host_and_guest,
    it_fails_a_call_with_the_error_of_its_call_hook,
    it_calls_the_call_hook_around_each_stretch_of_a_resumable_call,
    it_compiles_a_module_asynchronously_and_synchronously,
    it_refuses_bytes_that_are_not_a_module_with_a_compile_error,
    it_describes_the_imports_and_exports_of_a_module,
    it_refuses_an_import_of_the_wrong_type_with_a_link_error,
    it_instantiates_a_module_with_a_gc_global_and_an_exported_tag,
    it_loads_a_module_whose_internal_items_do_not_cross_its_boundary,
    it_describes_a_tag_at_the_boundary,
    it_links_a_tag_from_one_instance_into_another,
    it_enters_a_host_function_again_at_any_depth,
    it_calls_a_host_function_of_more_than_eight_parameters,
    it_traps_with_the_host_error_that_no_guest_can_catch,
    it_traps_with_a_type_mismatch_where_a_host_function_gives_a_wrong_result,
    it_reads_an_externref_the_guest_hands_back,
    it_calls_a_funcref_the_guest_hands_out,
    it_reads_an_i31ref,
    it_passes_a_gc_object_back_to_its_guest,
    it_passes_an_exnref_back_to_its_guest,
    it_keeps_the_references_the_host_holds_across_a_collection,
    it_refuses_host_suspension_where_it_is_not_declared,
    it_fails_with_an_exception_that_nothing_catches,
    it_resumes_calls_that_wait_at_once_in_any_order,
    it_runs_a_resumption_in_flight_to_its_next_stop_when_the_store_drops,
    it_finishes_a_resumable_call_that_does_not_suspend,
    it_traps_a_suspension_outside_a_resumable_call,
    it_traps_a_resumable_call_with_the_error_of_a_host_function,
    it_gives_the_store_back_when_the_future_of_a_resumption_drops,
    it_takes_up_a_resumption_whose_wait_dropped,
    it_reads_and_writes_a_memory,
    it_grows_a_memory_up_to_its_maximum,
    it_lends_the_bytes_of_a_range,
    it_refuses_a_range_outside_the_memory,
    it_copies_between_two_memories_of_one_store,
    it_addresses_a_64_bit_memory_with_the_same_methods,
);

wcmp_wasm_core_contract::contract_tests!(
    @backend Web::new;
    it_refuses_host_suspension_in_the_backend_itself,
);
