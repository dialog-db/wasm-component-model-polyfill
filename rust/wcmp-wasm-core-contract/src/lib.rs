#![warn(missing_docs)]

//! The backend contract tests of the runtime layer of the Wasm Component
//! Model Polyfill.
//!
//! Every backend of `wcmp_wasm_core` holds one contract. The tests here
//! state it once, written against the types of `wcmp_wasm_core` alone, and
//! every backend runs all of them with an engine of its own. A backend's
//! test file invokes [`contract_tests!`] with a function that makes its
//! engine:
//!
//! ```ignore
//! use wcmp_wasm_core::Engine;
//!
//! fn engine() -> Engine {
//!     Engine::with_backend(MyBackend::new())
//! }
//!
//! wcmp_wasm_core_contract::contract_tests!(engine);
//! ```
//!
//! The macro expands to one `#[wcmp_macros::test]` for each test of the
//! contract, so a backend runs every test, and a test added here reaches
//! every backend. The caller needs `tokio` natively and
//! `wasm-bindgen-test` on `wasm32`, as every test of the workspace does.
//! The tests build for both targets, because the browser backend runs them
//! in the web lane.
//!
//! A test that needs a capability runs only where the engine declares it.
//! Where the engine does not, the test checks what the contract says about
//! the missing capability, or passes without a check where the contract
//! says nothing.

mod boundary;
mod compile;
mod host_functions;
mod memory;
mod references;
mod support;
mod suspension;
mod tags;
mod traps;

pub use crate::boundary::{
    it_instantiates_a_module_with_a_gc_global_and_an_exported_tag,
    it_loads_a_module_whose_internal_items_do_not_cross_its_boundary,
};
pub use crate::compile::{
    it_compiles_a_module_asynchronously_and_synchronously,
    it_describes_the_imports_and_exports_of_a_module,
    it_refuses_an_import_of_the_wrong_type_with_a_link_error,
    it_refuses_bytes_that_are_not_a_module_with_a_compile_error,
};
pub use crate::host_functions::{
    it_calls_a_host_function_of_more_than_eight_parameters,
    it_enters_a_host_function_again_at_any_depth,
    it_traps_with_the_host_error_that_no_guest_can_catch,
};
pub use crate::memory::{
    it_addresses_a_64_bit_memory_with_the_same_methods,
    it_copies_between_two_memories_of_one_store, it_grows_a_memory_up_to_its_maximum,
    it_lends_the_bytes_of_a_range, it_reads_and_writes_a_memory,
    it_refuses_a_range_outside_the_memory,
};
pub use crate::references::{
    it_calls_a_funcref_the_guest_hands_out, it_passes_a_gc_object_back_to_its_guest,
    it_passes_an_exnref_back_to_its_guest, it_reads_an_externref_the_guest_hands_back,
    it_reads_an_i31ref,
};
pub use crate::suspension::{
    it_gives_the_store_back_when_the_future_of_a_resumption_drops,
    it_refuses_host_suspension_where_it_is_not_declared,
    it_resumes_calls_that_wait_at_once_in_any_order,
    it_runs_a_resumption_in_flight_to_its_next_stop_when_the_store_drops,
};
pub use crate::tags::{
    it_describes_a_tag_at_the_boundary, it_links_a_tag_from_one_instance_into_another,
};
pub use crate::traps::{
    it_fails_with_an_exception_that_nothing_catches,
    it_raises_each_core_trap_the_capabilities_permit,
};

/// The attribute each generated test carries, reached through this crate
/// so that a backend does not name it itself.
#[doc(hidden)]
pub mod __private {
    pub use wcmp_macros::test;
}

/// Expands to one test for each test of the backend contract, each run
/// with the engine that `$engine`, a function of no arguments that returns
/// an [`Engine`](wcmp_wasm_core::Engine), makes.
///
/// See the crate's documentation.
#[macro_export]
macro_rules! contract_tests {
    ($engine:path) => {
        $crate::contract_tests!(
            @each $engine;
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
            it_resumes_calls_that_wait_at_once_in_any_order,
            it_runs_a_resumption_in_flight_to_its_next_stop_when_the_store_drops,
            it_gives_the_store_back_when_the_future_of_a_resumption_drops,
            it_reads_and_writes_a_memory,
            it_grows_a_memory_up_to_its_maximum,
            it_lends_the_bytes_of_a_range,
            it_refuses_a_range_outside_the_memory,
            it_copies_between_two_memories_of_one_store,
            it_addresses_a_64_bit_memory_with_the_same_methods,
            it_raises_each_core_trap_the_capabilities_permit,
            it_fails_with_an_exception_that_nothing_catches,
        );
    };
    (@each $engine:path; $($case:ident),* $(,)?) => {
        $(
            #[$crate::__private::test]
            async fn $case() {
                $crate::$case(&$engine()).await;
            }
        )*
    };
}
