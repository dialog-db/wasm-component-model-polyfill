#![cfg(not(target_arch = "wasm32"))]
#![warn(missing_docs)]

//! The Wasmtime run of the toolchain compatibility scenarios.
//!
//! A scenario is a small program, or a small group of programs, with an
//! expectations file that lists the calls to make and the lines the
//! program prints. The Wasmtime run goes first, natively, as a step of
//! the build: it runs each compiled scenario through Wasmtime's
//! component API, judges what it saw against the expectations, and
//! writes the result as [`Observations`](wcmp_scenario::Observations).
//! The polyfill's subjects read those observations later and are judged
//! against them, so Wasmtime sets the behavior the polyfill must match.
//!
//! The Wasmtime here is the one this workspace pins, the same one the
//! polyfill's native backend links. A toolchain that brings a Wasmtime
//! of its own does not bring it here.
//!
//! # The build's layout
//!
//! A [`Scenario`] is read from two directories with one directory per
//! scenario in each, under the same name:
//!
//! - The scenario's sources, which hold its `expectations.txt` in the
//!   format of the scenario model.
//! - The compiled scenario, which holds three files per program:
//!   `<program>.status` with the compiler's exit status, `<program>.log`
//!   with the compiler's output, and `<program>.wasm` with the component
//!   when the program compiled. The calls of the expectations file name
//!   a component by its program's name.
//!
//! # What the run links
//!
//! The [`WasmtimeRun`] links the WASI Preview 2 and Preview 3 imports
//! from `wasmtime-wasi`, with standard output captured in memory, and
//! one fixed test interface, [`WasmtimeRun::TEST_INTERFACE`], whose one
//! function takes a string and returns it.

mod error;
mod program;
mod scenario;
mod value;
mod wasmtime_run;

pub use crate::error::{Error, Result};
pub use crate::program::Program;
pub use crate::scenario::Scenario;
pub use crate::wasmtime_run::WasmtimeRun;
