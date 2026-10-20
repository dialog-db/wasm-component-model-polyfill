// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The codegen corpus: `wcmp::bindgen!` over each entry of the WIT files
//! Wasmtime's macro tests use, at the pinned version
//! (`wasmtime/codegen/`, from `crates/component-macro/tests/codegen.rs`
//! at `v50.0.0`).
//!
//! Each entry generates under each set of options that compiles, on
//! both targets, which this file is built for. The sets that the
//! polyfill refuses for an entry are held to their refusals by the
//! macro crate's own tests (`rust/wcmp-bindgen/src/tests.rs`), which
//! read the same entries: Wasmtime's `async` and `async | store` sets
//! for an entry with a plain import, and its `tracing` set, which asks
//! for `ownership: Borrowing`.
//!
//! A new entry of the corpus has no line below, so the macro crate's
//! test that lists the corpus fails and names it until a person gives
//! it one.

#![cfg(test)]
#![allow(dead_code, unused_imports, clippy::all)]

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Generate the bindings of the corpus entry `$path` under each set of
/// options that compiles. `$imports` and `$exports` name one function
/// the entry imports and one it exports, for the set that puts
/// `trappable | exact` on one named function, or are empty strings for
/// an entry with none.
macro_rules! entry {
    ($name:ident, $path:literal, imports: $imports:tt, exports: $exports:tt) => {
        mod $name {
            /// Wasmtime's set with no options.
            mod sugar {
                wcmp::bindgen!(in $path);
            }
            /// Wasmtime's set with `store` on every import.
            mod imports_with_store {
                wcmp::bindgen!({ path: $path, imports: { default: store } });
            }
            /// Wasmtime's set with `anyhow: true` and `trappable` imports.
            mod with_anyhow {
                wcmp::bindgen!({
                    path: $path,
                    anyhow: true,
                    imports: { default: trappable },
                });
            }
            /// The polyfill's set with `tracing | verbose_tracing`, and no
            /// `Borrowing`.
            mod tracing {
                wcmp::bindgen!({
                    path: $path,
                    imports: { default: tracing | verbose_tracing },
                    exports: { default: tracing | verbose_tracing },
                });
            }
            /// The polyfill's set with `async | store` on every export.
            mod concurrent_exports {
                wcmp::bindgen!({ path: $path, exports: { default: async | store } });
            }
            /// The polyfill's set with `trappable | exact` on one named
            /// function.
            mod exact {
                entry!(@exact $path, $imports, $exports);
            }
        }
    };
    (@exact $path:literal, "", "") => {
        wcmp::bindgen!(in $path);
    };
    (@exact $path:literal, $imports:tt, "") => {
        wcmp::bindgen!({ path: $path, imports: { $imports: trappable | exact } });
    };
    (@exact $path:literal, "", $exports:tt) => {
        wcmp::bindgen!({ path: $path, exports: { $exports: trappable | exact } });
    };
    (@exact $path:literal, $imports:tt, $exports:tt) => {
        wcmp::bindgen!({
            path: $path,
            imports: { $imports: trappable | exact },
            exports: { $exports: trappable | exact },
        });
    };
}

entry!(char, "wasmtime/codegen/char.wit",
    imports: "foo:foo/chars.take-char", exports: "foo:foo/chars.take-char");
entry!(conventions, "wasmtime/codegen/conventions.wit",
    imports: "foo:foo/conventions.kebab-case", exports: "foo:foo/conventions.kebab-case");
entry!(dead_code, "wasmtime/codegen/dead-code.wit",
    imports: "a:b/interface-with-live-type.f", exports: "");
entry!(direct_import, "wasmtime/codegen/direct-import.wit", imports: "foo", exports: "");
entry!(empty, "wasmtime/codegen/empty.wit", imports: "", exports: "");
entry!(flags, "wasmtime/codegen/flags.wit",
    imports: "foo:foo/flegs.roundtrip-flag1", exports: "foo:foo/flegs.roundtrip-flag1");
entry!(floats, "wasmtime/codegen/floats.wit",
    imports: "foo:foo/floats.f32-param", exports: "foo:foo/floats.f32-param");
entry!(function_new, "wasmtime/codegen/function-new.wit", imports: "", exports: "new");
entry!(host_world, "wasmtime/codegen/host-world.wit", imports: "foo", exports: "");
entry!(implements, "wasmtime/codegen/implements.wit", imports: "hot-cache.get", exports: "");
entry!(integers, "wasmtime/codegen/integers.wit",
    imports: "foo:foo/integers.a1", exports: "foo:foo/integers.a1");
entry!(lists, "wasmtime/codegen/lists.wit",
    imports: "foo:foo/lists.list-u8-param", exports: "foo:foo/lists.list-u8-param");
entry!(many_arguments, "wasmtime/codegen/many-arguments.wit",
    imports: "foo:foo/manyarg.many-args", exports: "foo:foo/manyarg.many-args");
entry!(multiversion, "wasmtime/codegen/multiversion",
    imports: "my:dep/a@0.1.0.x", exports: "my:dep/a@0.1.0.x");
entry!(path1, "wasmtime/codegen/path1", imports: "", exports: "");
entry!(path2, "wasmtime/codegen/path2", imports: "", exports: "");
entry!(records, "wasmtime/codegen/records.wit",
    imports: "foo:foo/records.tuple-arg", exports: "foo:foo/records.tuple-arg");
entry!(rename, "wasmtime/codegen/rename.wit", imports: "foo:foo/red.foo", exports: "");
entry!(resources_export, "wasmtime/codegen/resources-export.wit",
    imports: "", exports: "foo:foo/simple-export.[method]a.method-a");
entry!(resources_import, "wasmtime/codegen/resources-import.wit",
    imports: "foo:foo/resources.[method]bar.method-a", exports: "");
entry!(share_types, "wasmtime/codegen/share-types.wit",
    imports: "http-fetch.fetch-request", exports: "http-handler.handle-request");
entry!(simple_functions, "wasmtime/codegen/simple-functions.wit",
    imports: "foo:foo/simple.f1", exports: "foo:foo/simple.f1");
entry!(simple_lists, "wasmtime/codegen/simple-lists.wit",
    imports: "foo:foo/simple-lists.simple-list1", exports: "foo:foo/simple-lists.simple-list1");
entry!(simple_wasi, "wasmtime/codegen/simple-wasi.wit",
    imports: "foo:foo/wasi-filesystem.create-directory-at", exports: "");
entry!(small_anonymous, "wasmtime/codegen/small-anonymous.wit",
    imports: "foo:foo/anon.option-test", exports: "foo:foo/anon.option-test");
entry!(smoke_default, "wasmtime/codegen/smoke-default.wit", imports: "", exports: "y");
entry!(smoke_export, "wasmtime/codegen/smoke-export.wit", imports: "", exports: "the-name.y");
entry!(smoke, "wasmtime/codegen/smoke.wit", imports: "imports.y", exports: "");
entry!(strings, "wasmtime/codegen/strings.wit",
    imports: "foo:foo/strings.a", exports: "foo:foo/strings.a");
entry!(unstable_features, "wasmtime/codegen/unstable-features.wit",
    imports: "foo:foo/the-interface.foo", exports: "");
entry!(unversioned_foo, "wasmtime/codegen/unversioned-foo.wit", imports: "foo:foo/a.g", exports: "");
entry!(use_paths, "wasmtime/codegen/use-paths.wit", imports: "foo:foo/a.a", exports: "");
entry!(variants, "wasmtime/codegen/variants.wit",
    imports: "foo:foo/variants.e1-arg", exports: "foo:foo/variants.e1-arg");
entry!(wat, "wasmtime/codegen/wat.wit", imports: "", exports: "");
entry!(worlds_with_types, "wasmtime/codegen/worlds-with-types.wit", imports: "", exports: "f");

/// The generated code of a few corpus entries, as committed: a change
/// to the generator fails these until the snapshots are written again
/// (see `wasmtime/README.md`).
const SNAPSHOTS: &[(&str, &str, &str)] = &[
    (
        "smoke",
        wcmp::bindgen!({ path: "wasmtime/codegen/smoke.wit", stringify: true }),
        include_str!("snapshots/smoke.txt"),
    ),
    (
        "records",
        wcmp::bindgen!({ path: "wasmtime/codegen/records.wit", stringify: true }),
        include_str!("snapshots/records.txt"),
    ),
    (
        "resources-import",
        wcmp::bindgen!({ path: "wasmtime/codegen/resources-import.wit", stringify: true }),
        include_str!("snapshots/resources-import.txt"),
    ),
    (
        "resources-export",
        wcmp::bindgen!({ path: "wasmtime/codegen/resources-export.wit", stringify: true }),
        include_str!("snapshots/resources-export.txt"),
    ),
];

#[wcmp_macros::test]
fn it_keeps_the_shape_of_the_generated_code() {
    let mut changed = Vec::new();
    for (name, generated, committed) in SNAPSHOTS {
        if generated != committed {
            println!("--- {name} ---\n{generated}\n--- end ---");
            changed.push(*name);
        }
    }
    assert!(
        changed.is_empty(),
        "the generated code of {changed:?} differs from the snapshot; if the change is \
         meant, write each printed block to `tests/snapshots/<name>.txt`"
    );
}
