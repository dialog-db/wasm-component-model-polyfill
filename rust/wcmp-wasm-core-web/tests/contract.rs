//! The backend contract of the runtime layer, on the browser backend.
//!
//! The browser backend does not make host functions yet: each needs a
//! generated wrapper module, so that a host error traps the guest. So the
//! tests of the contract that make a host function do not run here. They
//! are the two tests of host functions, and the test of a link error, whose
//! import of the wrong type is a host function. `web.rs` checks link errors
//! with externs of the browser's own.

#![cfg(target_arch = "wasm32")]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_web::Web;

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

fn engine() -> Engine {
    Engine::with_backend(Web::new())
}

wcmp_wasm_core_contract::contract_tests!(
    @each engine;
    it_compiles_a_module_asynchronously_and_synchronously,
    it_refuses_bytes_that_are_not_a_module_with_a_compile_error,
    it_describes_the_imports_and_exports_of_a_module,
    it_instantiates_a_module_with_a_gc_global_and_an_exported_tag,
    it_loads_a_module_whose_internal_items_do_not_cross_its_boundary,
    it_describes_a_tag_at_the_boundary,
    it_links_a_tag_from_one_instance_into_another,
    it_reads_an_externref_the_guest_hands_back,
    it_calls_a_funcref_the_guest_hands_out,
    it_reads_an_i31ref,
    it_passes_a_gc_object_back_to_its_guest,
    it_passes_an_exnref_back_to_its_guest,
    it_refuses_host_suspension_where_it_is_not_declared,
);
