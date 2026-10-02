// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Zena's compiler, as a component, on the polyfill.
//!
//! Each context makes one [`Compiler`] when it starts, from the
//! compiler component and the source bundle it fetches. The compiler
//! answers its `read-source` import from two places: the files of the
//! compile in progress, which a helper places beside its entry module,
//! and the source bundle. Nothing caches a compile's output: a context
//! compiles each component each time it needs one.

mod compile_error;
mod compile_request;
mod compiled;
#[allow(clippy::module_inception)]
mod compiler;

pub use compile_error::CompileError;
pub use compile_request::CompileRequest;
pub use compiled::Compiled;
pub use compiler::Compiler;
