#![warn(missing_docs)]

//! A polyfill that brings the WebAssembly Component Model (wasip3) to web
//! browsers where only Wasm Core is presently supported.
//!
//! See the design documents under `design/` for the project's scope, ecosystem
//! posture, and feature inventory:
//!
//! - PDD000 — product overview
//! - PDD001 — development environment
//! - PDD002 — ecosystem foundation ([`wasm_runtime_layer`] and
//!   [`wasm_component_layer`])
//! - PDD003 — compatibility outlook and implementation checklist
//! - PDD005 — library foundations (this slice)
//!
//! [`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer

mod backend;
mod engine;
mod error;
mod identifier;
mod store;

pub use crate::engine::Engine;
pub use crate::error::{Error, Result};
pub use crate::identifier::{IdentifierParseError, InterfaceIdentifier, PackageName};
pub use crate::store::Store;
