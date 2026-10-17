// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Names the directory `include_generated_code_from_file` and `debug`
//! write the generated code to: this crate's build output directory, as
//! Wasmtime's macro names its own.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR for a build script");
    println!("cargo:rustc-env=WCMP_BINDGEN_OUTPUT_DIR={out_dir}");
}
