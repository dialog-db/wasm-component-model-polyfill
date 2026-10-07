// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Sets the `wcmp_wast_runner` cfg where the build's environment names
//! `WCMP_WAST_RUNNER`.
//!
//! The cfg exports the entry the crate's conformance harness lowers the
//! store's record cap through, as Wasmtime's wast runner does. The
//! flake's test and lint builds set the variable. A crate that depends
//! on this one cannot turn the cfg on through a Cargo feature, so no
//! dependency graph turns it on by accident.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(wcmp_wast_runner)");
    println!("cargo::rerun-if-env-changed=WCMP_WAST_RUNNER");
    if std::env::var_os("WCMP_WAST_RUNNER").is_some() {
        println!("cargo::rustc-cfg=wcmp_wast_runner");
    }
}
