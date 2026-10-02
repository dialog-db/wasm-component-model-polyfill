// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tests of the demo lane. Each takes a browser session of its own
//! on the served demo, and answers why it failed, if it did.
//!
//! The tests reach into shadow roots with scripts, and define the
//! elements and routes of `fixtures/` through `window.demo`, the hooks
//! the page exposes for them.

use std::time::Duration;

use serde_json::Value;

use crate::browser::Browser;

mod app;
mod drawer;
mod elements;
mod platform;
mod routes;

/// One test of the lane.
pub struct Test {
    /// The test's name, which a filter matches.
    pub name: &'static str,
    /// The test.
    pub run: fn(&Browser) -> Result<(), String>,
}

/// A test of the lane, named after its function.
macro_rules! test {
    ($module:ident :: $name:ident) => {
        Test {
            name: stringify!($name),
            run: $module::$name,
        }
    };
}

/// Every test of the lane, in the order the report lists them.
pub static TESTS: &[Test] = &[
    test!(platform::it_loads_the_page_and_the_service_worker_controls_it),
    test!(platform::it_shows_a_skeleton_until_the_elements_are_defined),
    test!(platform::it_stops_the_service_worker_and_the_next_request_starts_it),
    test!(platform::it_reads_each_network_response_with_its_headers),
    test!(platform::it_sets_the_color_scheme),
    test!(platform::it_clears_indexed_db),
    test!(platform::it_loads_a_static_file_through_the_service_worker_from_the_network),
    test!(platform::it_compiles_and_runs_a_program_on_the_page),
    test!(platform::it_compiles_and_runs_a_program_in_the_service_worker),
    test!(platform::it_reports_the_file_and_line_of_a_compile_error),
    test!(platform::it_compiles_a_program_that_imports_a_file_beside_it),
    test!(platform::it_records_the_spans_of_the_page_as_performance_measures),
    test!(platform::it_records_no_spans_when_the_trace_parameter_is_off),
    test!(elements::it_gives_create_the_attributes_set_before_the_element_connects),
    test!(elements::it_gives_an_element_that_connects_again_a_new_id),
    test!(elements::it_serves_fifty_elements_of_a_tag_with_one_instance),
    test!(elements::it_answers_the_diagnostics_of_a_source_that_does_not_compile),
    test!(elements::it_changes_nodes_in_place_when_an_attribute_changes),
    test!(elements::it_keeps_keyed_nodes_when_a_list_reverses),
    test!(elements::it_applies_the_styles_an_element_returns),
    test!(elements::it_makes_a_child_element_with_its_own_shadow_root),
    test!(elements::it_calls_the_handler_a_view_names_for_a_click),
    test!(elements::it_sends_a_child_event_to_its_parent),
    test!(elements::it_runs_the_calls_of_one_element_in_order),
    test!(elements::it_runs_calls_of_two_elements_of_a_tag_at_the_same_time),
    test!(elements::it_renders_and_handles_a_click_in_the_class_and_the_function_form),
    test!(elements::it_renders_a_function_form_element_without_an_event_function),
    test!(elements::it_shows_an_error_card_when_an_element_traps_and_works_after_a_restart),
    test!(elements::it_fails_a_call_outside_the_http_subset),
    test!(routes::it_answers_a_route_with_its_pattern_and_parameters),
    test!(routes::it_sends_every_method_with_headers_and_a_body_through_wasi_http),
    test!(routes::it_answers_500_for_a_route_that_traps_and_compiles_it_again),
    test!(routes::it_answers_500_with_the_diagnostics_of_a_route_that_does_not_compile),
    test!(routes::it_answers_each_request_of_the_route_tables),
    test!(routes::it_enforces_the_rules_of_the_todo_model_through_the_routes),
    test!(app::it_adds_toggles_toggles_all_and_deletes_todos),
    test!(app::it_edits_a_todo_with_enter_blur_and_escape),
    test!(app::it_deletes_a_todo_whose_title_an_edit_empties),
    test!(app::it_counts_the_active_todos),
    test!(app::it_filters_by_the_url_hash),
    test!(app::it_clears_the_completed_todos_and_shows_the_button_only_for_them),
    test!(app::it_keeps_the_list_after_a_reload),
    test!(app::it_keeps_the_list_after_the_worker_stops_and_the_page_reloads),
    test!(app::it_calls_the_routes_through_wasi_http_and_the_service_worker),
    test!(app::it_follows_the_color_scheme),
    test!(app::it_reads_the_color_tokens_in_every_element),
    test!(drawer::it_lists_every_source_with_its_timings),
    test!(drawer::it_shows_a_new_compile_of_a_route_after_the_worker_stops),
    test!(drawer::it_shows_one_instance_and_the_connected_elements),
    test!(drawer::it_applies_an_element_edit_in_place),
    test!(drawer::it_keeps_the_last_good_element_after_a_failed_edit),
    test!(drawer::it_falls_back_to_the_shipped_element_when_an_edit_fails_at_boot),
    test!(drawer::it_resets_an_element_to_the_shipped_source),
    test!(drawer::it_restarts_an_element_after_a_trap),
    test!(drawer::it_applies_a_route_edit_in_the_service_worker),
    test!(drawer::it_answers_500_for_a_saved_route_that_traps),
    test!(drawer::it_keeps_the_old_route_after_a_failed_edit),
    test!(drawer::it_resets_a_route_to_the_shipped_source),
];

/// Fail with `message` unless `condition` holds.
fn check(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if condition { Ok(()) } else { Err(message()) }
}

/// `text` as a JavaScript string literal.
fn js(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// Define the element `tag` from `source` through `window.demo`.
fn define(browser: &Browser, tag: &str, source: &str) -> Result<(), String> {
    browser.eval(&format!(
        "await window.demo.define({}, {}); return true;",
        js(tag),
        js(source)
    ))?;
    Ok(())
}

/// Define the route `pattern` from `source` through `window.demo`.
fn define_route(browser: &Browser, pattern: &str, source: &str) -> Result<(), String> {
    browser.eval(&format!(
        "await window.demo.defineRoute({}, {}); return true;",
        js(pattern),
        js(source)
    ))?;
    Ok(())
}

/// The status `window.demo.elements()` answers for `tag`.
fn element_status(browser: &Browser, tag: &str) -> Result<Value, String> {
    let statuses = browser.eval("return window.demo.elements();")?;
    statuses
        .as_array()
        .and_then(|statuses| statuses.iter().find(|status| status["tag"] == tag).cloned())
        .ok_or_else(|| format!("no status for <{tag}> in {statuses}"))
}

/// The status `window.demo.routes()` answers for `pattern`.
fn route_status(browser: &Browser, pattern: &str) -> Result<Value, String> {
    let statuses = browser.eval("return await window.demo.routes();")?;
    statuses
        .as_array()
        .and_then(|statuses| {
            statuses
                .iter()
                .find(|status| status["pattern"] == pattern)
                .cloned()
        })
        .ok_or_else(|| format!("no status for {pattern} in {statuses}"))
}

/// Wait `millis` milliseconds.
fn pause(millis: u64) {
    Browser::sleep(Duration::from_millis(millis));
}
