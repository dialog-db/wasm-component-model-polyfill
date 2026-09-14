//! The polyfill's component executor.
//!
//! Component-level work in the polyfill — instantiation, export
//! invocation, the canonical-ABI runtime — is implemented on top of
//! [`wasm_runtime_layer`]'s generic core-Wasm abstractions. The
//! executor consumes a flattened, sequenced IR (the same shape
//! `wasmtime_environ::component::Component` carries) and drives
//! `wasm_runtime_layer::Module::new` and
//! `wasm_runtime_layer::Instance::new` per `(core module ...)` and
//! `(core instance N (instantiate $module ...))` directive.
//!
//! IR construction is shared across targets: `wasmtime_environ`'s
//! component `Translator` runs on every supported target, including
//! `wasm32-unknown-unknown`. The crate's `compile` feature, despite
//! its name, does not pull in Cranelift codegen — it only enables
//! `wasm-encoder`, `wasmprinter`, and the `gimli`/`object` write
//! paths, all of which build on `wasm32-unknown-unknown`.
//!
//! [`wasm_runtime_layer`]: https://docs.rs/wasm_runtime_layer

mod instantiate;
mod translate;

pub mod intrinsics;
pub mod ir;
pub mod trampoline;

pub use instantiate::instantiate;
pub use translate::translate;
