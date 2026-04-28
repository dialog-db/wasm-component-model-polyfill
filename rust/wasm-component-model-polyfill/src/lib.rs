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
