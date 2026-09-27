#![warn(missing_docs)]

//! The scenario model of the toolchain compatibility tests: what a
//! scenario expects, what the Wasmtime run observed, where a subject
//! stopped, and the rules that judge a subject.
//!
//! A scenario is a small program, or a small group of programs, run by
//! several subjects. The Wasmtime run goes first, natively, and sets the
//! behavior the polyfill must match. The polyfill then runs the same
//! scenario in the browser and natively. Each subject stops at one
//! [`Stage`], and a [`Verdict`] pairs that stage with the text a person
//! reads to learn why.
//!
//! A contributor writes the [`Expectations`] by hand: the calls in
//! order, each with the results it returns, a failure, or no outcome at
//! all, and the lines the scenario prints. A subject that reaches its
//! calls reports a [`Run`], one [`Outcome`] per entry plus the lines it
//! printed. The Wasmtime run is judged against the expectations, and
//! what it saw becomes its [`Observations`]: a build product that the
//! polyfill subjects read and judge themselves against.
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
//!   is one call. `typed` asks for a typed call. The component is one
//!   word, and the export runs up to the opening parenthesis, so an
//!   interface export such as `local:demo/api#greet` needs no quoting.
//!   The outcome is `fail`, optionally followed by a quoted message, or
//!   the results: `()` for none, one value, or several in parentheses.
//! - `output "<line>"` is the next line the scenario prints.
//! - `stage <stage> ["<reason>"]` appears in observations only, once,
//!   and holds the Wasmtime run's verdict.
//!
//! A value is `true` or `false`, a number with its type as a suffix
//! (`7u8`, `-3s32`, `1.5f64`, `NaNf32`, `-inff64`), a character in
//! single quotes, or a string in double quotes. Quoted text takes the
//! escapes `\\`, `\"`, `\'`, `\n`, `\r`, `\t`, `\0`, and `\u{…}`.

mod call;
mod entry;
mod error;
mod expectations;
mod judge;
mod observation;
mod observations;
mod outcome;
mod run;
mod stage;
mod syntax;
mod value;
mod verdict;

pub use crate::call::Call;
pub use crate::entry::Entry;
pub use crate::error::{Error, Result};
pub use crate::expectations::Expectations;
pub use crate::observation::Observation;
pub use crate::observations::Observations;
pub use crate::outcome::Outcome;
pub use crate::run::Run;
pub use crate::stage::Stage;
pub use crate::value::Value;
pub use crate::verdict::Verdict;

// The model's own unit tests reach a browser in the web lane.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
