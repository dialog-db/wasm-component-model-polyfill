// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tests of what the macro refuses, which fail the compile and so are
//! read from the expansion itself rather than from a crate that
//! invokes the macro.

use std::path::Path;

use proc_macro2::TokenStream;
use quote::quote;

use super::expand;

/// A world of one interface, for the invocations below.
const WORLD: &str = "
    package test:refusals;

    interface log {
        record entry { message: string }
        write: func(entry: entry);
    }

    world refusals {
        import log;
    }
";

/// The message the expansion of `input` fails with.
fn refusal(input: TokenStream) -> String {
    match expand(input, Path::new(".")) {
        Ok(code) => panic!("the invocation expands: {code}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn it_refuses_wasmtime_crate_and_names_wcmp_crate() {
    let message = refusal(quote!({ inline: #WORLD, wasmtime_crate: wasmtime }));
    assert!(
        message.contains("`wasmtime_crate`") && message.contains("`wcmp_crate: path`"),
        "the refusal names the option and its replacement: {message}"
    );
}

#[test]
fn it_refuses_an_imports_rule_that_matches_no_function() {
    let message = refusal(quote!({
        inline: #WORLD,
        imports: { "test:refusals/log.missing": trappable },
    }));
    assert!(
        message.contains("unused `imports` rules") && message.contains("test:refusals/log.missing"),
        "the refusal names the rule: {message}"
    );
}

#[test]
fn it_refuses_the_async_flag_on_a_plain_import() {
    for input in [
        quote!({ inline: #WORLD, imports: { default: async } }),
        quote!({ inline: #WORLD, imports: { default: async | store } }),
    ] {
        let message = refusal(input);
        assert!(
            message.contains("`async` flag")
                && message.contains("`write`")
                && message.contains("holds the store across an await")
                && message.contains("`async func`"),
            "the refusal names the flag and the function, says why, and suggests \
             `async func`: {message}"
        );
    }
}
