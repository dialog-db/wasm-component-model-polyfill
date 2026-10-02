// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(missing_docs)]

//! The scenario model of the toolchain compatibility tests: what a
//! scenario expects, what the Wasmtime run observed, where a subject
//! stopped, and the rules that judge a subject.
//!
//! A scenario is a small program, or a small group of programs, run by
//! several subjects. The Wasmtime run goes first, natively, and sets the
//! behavior the polyfill must match. The polyfill then runs the same
//! scenario in the browser, and natively over the Wasmtime and the Wasmi
//! backends of its runtime layer. Each subject stops at one
//! [`Stage`], and a [`Verdict`] pairs that stage with the text a person
//! reads to learn why. A [`Report`] puts one [`Subject`]'s verdict on
//! one scenario on one line.
//!
//! A [`Record`] is the committed stage of every scenario for every
//! subject, made from one revision of the toolchain. It is the gate of
//! a run: every [`Difference`] between the two fails the run, and only
//! the stages are compared. A [`Compatibility`] report prints a run's
//! reports for a person: one entry per scenario, and under it where
//! each subject stopped and why.
//!
//! A contributor writes the [`Expectations`] by hand: the calls in
//! order, each with the results it returns, a failure, or no outcome at
//! all, and the lines the scenario prints. A subject that reaches its
//! calls reports a [`Run`], one [`Outcome`] per entry plus the lines it
//! printed. The Wasmtime run is judged against the expectations, and
//! what it saw becomes its [`Observations`]: a build product that the
//! polyfill subjects read and judge themselves against.
//!
//! A scenario with several components also has a [`Wiring`]: one
//! [`Link`] per import that another component's export satisfies, made
//! at run time by the runner or ahead of time by composition, as its
//! [`Linking`] says. The wiring orders the components for run-time
//! linking, each exporter before its importer. Its file has a format of
//! its own, which [`Wiring`] describes.
//!
//! Nothing here depends on the toolchain that built a scenario, on
//! Wasmtime, or on the polyfill, so the same model serves the Wasmtime
//! run and the polyfill runner on both targets.
//!
//! # The text format
//!
//! Expectations and observations share one line-oriented format. A
//! blank line or a line whose first non-blank character is `#` is
//! ignored. Every other line starts with a keyword:
//!
//! ```text
//! # A call with its results, a call that must fail, and a call with no
//! # outcome, which the Wasmtime run decides.
//! call main add(1u32, 2u32) -> 3u32
//! call typed main greet("world") -> "hello, world"
//! call main boom() -> fail
//! call main boom()
//! output "hello, world"
//! ```
//!
//! - `call [typed] <component> <export>(<arguments>) [-> <outcome>]`
//!   is one call. `typed` asks for a typed call, which a runner makes
//!   only for a signature in the closed set of [`TypedSignature`],
//!   through the Rust types of [`Typed`]. The component is one
//!   word, and the export runs up to the opening parenthesis, so an
//!   interface export such as `local:demo/api#greet` needs no quoting.
//!   The outcome is `fail`, optionally followed by a quoted message, or
//!   the results: `()` for none, one value, or several in parentheses.
//!   A call that returns a component, as a compile does, has the outcome
//!   `component`, optionally followed by the name a later call loads it
//!   as and by its `sha256:` digest. A call that returns an error string
//!   has the outcome `err "<text>"` for the whole text, or
//!   `err containing "<text>"` for a part of it.
//! - `output "<line>"` is the next line the scenario prints.
//! - `stage <stage> ["<reason>"]` appears in observations only, once,
//!   and holds the Wasmtime run's verdict.
//!
//! A value is `true` or `false`, a number with its type as a suffix
//! (`7u8`, `-3s32`, `1.5f64`, `NaNf32`, `-inff64`), a character in
//! single quotes, or a string in double quotes. Quoted text takes the
//! escapes `\\`, `\"`, `\'`, `\n`, `\r`, `\t`, `\0`, and `\u{…}`.

mod call;
mod compatibility;
mod difference;
mod entry;
mod error;
mod expectations;
mod judge;
mod link;
mod linking;
mod observation;
mod observations;
mod outcome;
mod record;
mod report;
mod run;
mod source_bundle;
mod stage;
mod subject;
mod syntax;
mod typed;
mod typed_signature;
mod value;
mod value_type;
mod verdict;
mod wiring;

pub use crate::call::Call;
pub use crate::compatibility::Compatibility;
pub use crate::difference::Difference;
pub use crate::entry::Entry;
pub use crate::error::{Error, Result};
pub use crate::expectations::Expectations;
pub use crate::link::Link;
pub use crate::linking::Linking;
pub use crate::observation::Observation;
pub use crate::observations::Observations;
pub use crate::outcome::{Outcome, digest};
pub use crate::record::Record;
pub use crate::report::Report;
pub use crate::run::Run;
pub use crate::source_bundle::SourceBundle;
pub use crate::stage::Stage;
pub use crate::subject::Subject;
pub use crate::typed::Typed;
pub use crate::typed_signature::TypedSignature;
pub use crate::value::Value;
pub use crate::value_type::ValueType;
pub use crate::verdict::Verdict;
pub use crate::wiring::Wiring;

/// The fixed test interface every subject supplies to a scenario. Its
/// one function, [`TEST_FUNCTION`], takes a string and returns it.
pub const TEST_INTERFACE: &str = "wcmp:scenario/host";

/// The function of [`TEST_INTERFACE`]: `echo: func(text: string) ->
/// string`.
pub const TEST_FUNCTION: &str = "echo";

/// The interface of the compiler component's one host import. Every
/// subject supplies it to a scenario, answered from the toolchain's
/// [`SourceBundle`].
pub const COMPILER_HOST_INTERFACE: &str = "wcmp:zena-compiler/host";

/// The function of [`COMPILER_HOST_INTERFACE`]: `read-source: func(path:
/// string) -> option<string>`, the text of a file the compile reads, or
/// none.
pub const READ_SOURCE: &str = "read-source";

// The model's own unit tests reach a browser in the web lane.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
