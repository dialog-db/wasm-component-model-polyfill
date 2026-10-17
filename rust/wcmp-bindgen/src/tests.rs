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

#[test]
fn it_refuses_a_trappable_error_type_that_names_no_wit_type() {
    let message = refusal(quote!({
        inline: #WORLD,
        trappable_error_type: { "test:refusals/log.missing-error" => MyError },
    }));
    assert!(
        message.contains("trappable_error_type")
            && message.contains("test:refusals/log.missing-error"),
        "the refusal names the entry: {message}"
    );
}

/// A world with a resource, for the refusals of its destructor.
const RESOURCE_WORLD: &str = "
    package test:refusals;

    interface files {
        resource file {
            read: func() -> u32;
        }
    }

    world refusals {
        import files;
    }
";

#[test]
fn it_refuses_a_with_key_that_matches_nothing() {
    let message = refusal(quote!({
        inline: #RESOURCE_WORLD,
        with: { "test:refusals/missing": crate::missing },
    }));
    assert!(
        message.contains("unused `with`") && message.contains("test:refusals/missing"),
        "the refusal names the key: {message}"
    );
}

#[test]
fn it_refuses_the_async_flag_on_a_destructor() {
    let message = refusal(quote!({
        inline: #RESOURCE_WORLD,
        imports: { "test:refusals/files.[drop]file": async },
    }));
    assert!(
        message.contains("`async` flag") && message.contains("`[drop]file`"),
        "the refusal names the rule: {message}"
    );
}

#[test]
fn it_refuses_borrowing_ownership() {
    let message = refusal(quote!({
        inline: #WORLD,
        ownership: Borrowing { duplicate_if_necessary: true },
    }));
    assert!(
        message.contains("`ownership: Borrowing") && message.contains("lifetime parameters"),
        "the refusal names the option and the reason: {message}"
    );
}

#[test]
fn it_refuses_named_imports() {
    let message = refusal(quote!({
        inline: #WORLD,
        named_imports: { "test:refusals/log": crate::Id },
    }));
    assert!(
        message.contains("`named_imports`") && message.contains("`(implements …)`"),
        "the refusal names the option and the reason: {message}"
    );
}

#[test]
fn it_refuses_include_component_type() {
    let message = refusal(quote!({ inline: #WORLD, include_component_type: true }));
    assert!(
        message.contains("`include_component_type`") && message.contains("encoded component type"),
        "the refusal names the option and the reason: {message}"
    );
}

/// The parser of Wasmtime's options at the pinned version.
const WASMTIME_BINDGEN: &str = include_str!("../../wcmp-bindgen-tests/wasmtime/bindgen.rs.txt");

/// The flags of Wasmtime's macro at the pinned version.
const WASMTIME_CONFIG: &str = include_str!("../../wcmp-bindgen-tests/wasmtime/config.rs.txt");

/// The options the parser of Wasmtime's macro accepts: each keyword its
/// `Opt` parser peeks for.
fn wasmtime_options() -> Vec<String> {
    let start = WASMTIME_BINDGEN
        .find("impl Parse for Opt")
        .expect("the pinned parser has `Opt`");
    let end = start
        + WASMTIME_BINDGEN[start..]
            .find("fn trappable_error_field_parse")
            .expect("the parser of one option ends");
    let parser = &WASMTIME_BINDGEN[start..end];
    parser
        .match_indices("l.peek(kw::")
        .map(|(at, prefix)| {
            let rest = &parser[at + prefix.len()..];
            rest[..rest.find(')').expect("a keyword ends")].to_owned()
        })
        .collect()
}

/// The flags of Wasmtime's macro: each constant of `FunctionFlags`.
fn wasmtime_flags() -> Vec<String> {
    WASMTIME_CONFIG
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("const ")?;
            let (name, value) = line.split_once(" = ")?;
            value.starts_with("1 <<").then(|| name.to_owned())
        })
        .collect()
}

/// The options and the flags of Wasmtime's macro that `options` and
/// `flags` have no entry for.
fn unclassified(options: &[&str], flags: &[&str]) -> Vec<String> {
    let options = wasmtime_options()
        .into_iter()
        .filter(|option| !options.contains(&option.as_str()))
        .map(|option| format!("option `{option}`"));
    let flags = wasmtime_flags()
        .into_iter()
        .filter(|flag| !flags.contains(&flag.as_str()))
        .map(|flag| format!("flag `{flag}`"));
    options.chain(flags).collect()
}

#[test]
fn it_has_a_ledger_entry_for_each_option_and_flag_of_wasmtimes_macro() {
    let options = crate::ledger::OPTIONS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let flags = crate::ledger::FLAGS
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<Vec<_>>();
    assert_eq!(
        wasmtime_options().len(),
        19,
        "Wasmtime's parser accepts 19 options"
    );
    assert_eq!(wasmtime_flags().len(), 6, "Wasmtime's macro has six flags");
    assert_eq!(
        unclassified(&options, &flags),
        Vec::<String>::new(),
        "each option and flag of Wasmtime's macro has an entry in the ledger"
    );
}

#[test]
fn it_names_an_option_the_ledger_has_no_entry_for() {
    let options = crate::ledger::OPTIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| *name != "anyhow")
        .collect::<Vec<_>>();
    let flags = crate::ledger::FLAGS
        .iter()
        .map(|(name, _, _)| *name)
        .filter(|name| *name != "EXACT")
        .collect::<Vec<_>>();
    assert_eq!(
        unclassified(&options, &flags),
        ["option `anyhow`", "flag `EXACT`"],
        "an option or a flag without an entry fails the test, named"
    );
}

/// The directory of the pinned corpus, which nextest's workspace remap
/// gives the tests at run time.
fn wasmtime_dir() -> std::path::PathBuf {
    let manifest =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo names the crate's directory");
    std::path::PathBuf::from(manifest).join("../wcmp-bindgen-tests/wasmtime")
}

/// Each entry of the corpus, as `tests/codegen.rs` in
/// `wcmp-bindgen-tests` lists it, with whether it has a plain import or
/// a resource whose destructor the `async` sets give the flag.
const CORPUS: &[(&str, bool)] = &[
    ("char.wit", true),
    ("conventions.wit", true),
    ("dead-code.wit", true),
    ("direct-import.wit", true),
    ("empty.wit", false),
    ("flags.wit", true),
    ("floats.wit", true),
    ("function-new.wit", false),
    ("host-world.wit", true),
    ("implements.wit", true),
    ("integers.wit", true),
    ("lists.wit", true),
    ("many-arguments.wit", true),
    ("multiversion", true),
    ("path1", false),
    ("path2", false),
    ("records.wit", true),
    ("rename.wit", true),
    ("resources-export.wit", true),
    ("resources-import.wit", true),
    ("share-types.wit", true),
    ("simple-functions.wit", true),
    ("simple-lists.wit", true),
    ("simple-wasi.wit", true),
    ("small-anonymous.wit", true),
    ("smoke-default.wit", false),
    ("smoke-export.wit", false),
    ("smoke.wit", true),
    ("strings.wit", true),
    ("unstable-features.wit", true),
    ("unversioned-foo.wit", true),
    ("use-paths.wit", true),
    ("variants.wit", true),
    ("wat.wit", false),
    ("worlds-with-types.wit", false),
];

/// What the invocation of the corpus entry `entry` with `options`
/// gives: the generated code, or the refusal.
fn corpus(entry: &str, options: TokenStream) -> Result<TokenStream, String> {
    let path = format!("codegen/{entry}");
    let input = quote!({ path: #path, #options });
    expand(input, &wasmtime_dir()).map_err(|error| error.to_string())
}

#[test]
fn it_lists_every_entry_of_the_corpus() {
    let mut found = std::fs::read_dir(wasmtime_dir().join("codegen"))
        .expect("the pinned corpus")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    found.sort();
    let listed = CORPUS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        found, listed,
        "each corpus entry has an expected result here and a line in \
         `wcmp-bindgen-tests/tests/codegen.rs`; a new entry is named by this failure"
    );
    assert_eq!(found.len(), 35, "the corpus has 35 entries");
}

#[test]
fn it_refuses_the_async_sets_for_an_entry_with_a_plain_import() {
    for options in [
        quote!(imports: { default: async }, exports: { default: async },),
        quote!(imports: { default: async | store }, exports: { default: async | store },),
    ] {
        for (entry, refused) in CORPUS {
            match corpus(entry, options.clone()) {
                Err(message) if *refused => assert!(
                    message.contains("`async` flag") && message.contains("is not supported"),
                    "{entry} is refused for its `async` flag: {message}"
                ),
                Ok(_) if !*refused => {}
                other => panic!(
                    "{entry} under {options}: expected {}, got {:?}",
                    if *refused { "the refusal" } else { "code" },
                    other.map(|_| "code")
                ),
            }
        }
    }
}

#[test]
fn it_refuses_the_tracing_set_for_its_borrowing() {
    for (entry, _) in CORPUS {
        let message = corpus(
            entry,
            quote! {
                imports: { default: tracing | verbose_tracing },
                exports: { default: tracing | verbose_tracing },
                ownership: Borrowing { duplicate_if_necessary: true },
            },
        )
        .expect_err("`Borrowing` is refused");
        assert!(
            message.contains("`ownership: Borrowing"),
            "{entry} is refused for `Borrowing`: {message}"
        );
    }
}
