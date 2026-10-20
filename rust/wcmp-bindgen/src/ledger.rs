// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The option ledger: how the macro treats each option and each flag of
//! Wasmtime's `bindgen!`.

/// How the macro treats one option or one flag of Wasmtime's macro.
///
/// The ledger test reads the two tables below against Wasmtime's own
/// option set; the macro itself reads the reasons of the refusals.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The same meaning as in Wasmtime.
    Matched,
    /// The same meaning as in Wasmtime natively, and no effect in the
    /// browser.
    MatchedNatively,
    /// Accepted, with no effect.
    NoEffect,
    /// Refused, for the reason given.
    Refused(&'static str),
}

/// Each option Wasmtime's macro parses at `v50.0.0`
/// (`crates/component-macro/src/bindgen.rs`), by its keyword.
#[cfg_attr(not(test), allow(dead_code))]
pub const OPTIONS: &[(&str, Entry)] = &[
    ("world", Entry::Matched),
    ("path", Entry::Matched),
    ("inline", Entry::Matched),
    ("interfaces", Entry::Matched),
    ("with", Entry::Matched),
    ("imports", Entry::Matched),
    ("exports", Entry::Matched),
    ("trappable_error_type", Entry::Matched),
    ("additional_derives", Entry::Matched),
    ("skip_mut_forwarding_impls", Entry::Matched),
    ("stringify", Entry::Matched),
    ("debug", Entry::Matched),
    ("include_generated_code_from_file", Entry::Matched),
    ("anyhow", Entry::Matched),
    ("require_store_data_send", Entry::MatchedNatively),
    // `Owning` is matched; `Borrowing { … }` is refused, see
    // `BORROWING`.
    ("ownership", Entry::Matched),
    ("named_imports", Entry::Refused(NAMED_IMPORTS)),
    (
        "include_component_type",
        Entry::Refused(INCLUDE_COMPONENT_TYPE),
    ),
    ("wasmtime_crate", Entry::Refused(WASMTIME_CRATE)),
];

/// Each flag Wasmtime's `imports` and `exports` options give a function
/// at `v50.0.0` (`crates/wit-bindgen/src/config.rs`), by its
/// constant, with its entry on imports and on exports.
///
/// A flag's entry depends on what the function is too: `async` on a
/// WIT `async func` is the default, on a plain import it is refused
/// (see [`ASYNC_IMPORT`]), and on a destructor it is refused (see
/// [`ASYNC_DROP`]). `store` on a destructor is matched.
#[cfg_attr(not(test), allow(dead_code))]
pub const FLAGS: &[(&str, Entry, Entry)] = &[
    ("ASYNC", Entry::Refused(ASYNC_IMPORT), Entry::NoEffect),
    ("TRAPPABLE", Entry::Matched, Entry::Matched),
    ("STORE", Entry::Matched, Entry::NoEffect),
    ("TRACING", Entry::Matched, Entry::Matched),
    ("VERBOSE_TRACING", Entry::Matched, Entry::Matched),
    ("EXACT", Entry::Matched, Entry::Matched),
];

/// Why `ownership: Borrowing { … }` is refused.
pub const BORROWING: &str = "the polyfill's derived types have no lifetime parameters, so a \
     generated type cannot borrow its fields; use `ownership: Owning`, the default";

/// Why `named_imports` is refused.
pub const NAMED_IMPORTS: &str = "it needs linking that reads the `(implements …)` annotations of a component, which the \
     polyfill's linker does not";

/// Why `include_component_type` is refused.
pub const INCLUDE_COMPONENT_TYPE: &str =
    "the polyfill generates no encoded component type for its bindings";

/// Why `wasmtime_crate` is refused.
pub const WASMTIME_CRATE: &str = "the polyfill's macro emits paths under `wcmp`; use \
     `wcmp_crate: path` to name the crate that re-exports the polyfill";

/// Why `async` on a plain import is refused.
pub const ASYNC_IMPORT: &str = "the polyfill has no host function that holds the store across \
     an await, which Wasmtime's `func_wrap_async` is; declare the function `async func` in the \
     WIT to serve it concurrently, or drop the flag to serve it synchronously";

/// Why `async` on a destructor is refused.
pub const ASYNC_DROP: &str = "a resource's destructor is synchronous in the polyfill; use \
     `store` to give `drop` an `Access`, or drop the flag";
