// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the drawer shows of a tag.

/// What the drawer shows of a tag.
#[derive(Debug, Clone, Default)]
pub struct TagStatus {
    /// The tag.
    pub tag: String,
    /// The source the tag runs, or tried to run last.
    pub source: String,
    /// The time of the last compile, in milliseconds.
    pub compile_ms: Option<f64>,
    /// The time of the last instantiation, in milliseconds.
    pub instantiate_ms: Option<f64>,
    /// The diagnostics of the last compile, when it failed.
    pub diagnostics: Option<String>,
    /// The trap that poisoned the tag's store, if one did.
    pub trapped: Option<String>,
    /// The elements of the tag in the document.
    pub connected: usize,
    /// The instances of the tag's component: one while it runs.
    pub instances: usize,
    /// How many times the page started the tag's component from a
    /// compile.
    pub starts: u32,
}
