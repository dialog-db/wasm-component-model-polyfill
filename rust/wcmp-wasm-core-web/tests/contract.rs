//! The backend contract of the runtime layer, on the browser backend.
//!
//! Every test of the contract runs here. The trap fixture runs with the
//! one allowance below.

#![cfg(target_arch = "wasm32")]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_contract::AmbiguousTrap;
use wcmp_wasm_core_web::Web;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

/// The traps of the fixture that the browser's engine words with a message
/// of more than one kind, which the backend reports as `Other` with that
/// message.
///
/// V8 words a truncation of a NaN, which Wasmtime calls
/// `BadConversionToInteger`, and a truncation of a float outside the range
/// of the integer, which Wasmtime calls `IntegerOverflow`, with one message,
/// the template `WasmTrapFloatUnrepresentable` of
/// <https://github.com/v8/v8/blob/6cb2fb511f6f0e7930a51cbd08ee15b693bf3610/src/common/message-template.h>.
/// The test fails as stale once the backend reports `BadConversionToInteger`
/// for the trap.
const AMBIGUOUS: &[AmbiguousTrap] = &[AmbiguousTrap {
    export: "bad_conversion_to_integer",
    message: "float unrepresentable in integer range",
}];

#[wcmp_macros::test]
async fn it_raises_each_core_trap_the_capabilities_permit() {
    wcmp_wasm_core_contract::it_raises_each_core_trap_allowing(&engine(), AMBIGUOUS).await;
}

wcmp_wasm_core_contract::contract_tests!(
    @each engine;
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
    it_reads_an_externref_the_guest_hands_back,
    it_calls_a_funcref_the_guest_hands_out,
    it_reads_an_i31ref,
    it_passes_a_gc_object_back_to_its_guest,
    it_passes_an_exnref_back_to_its_guest,
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
