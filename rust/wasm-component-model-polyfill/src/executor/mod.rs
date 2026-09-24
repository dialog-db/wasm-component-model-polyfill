//! The polyfill's component executor.
//!
//! Component-level work in the polyfill — instantiation, export
//! invocation, the canonical-ABI runtime — is implemented on top of
//! [`wasm_runtime_layer`]'s generic core-Wasm abstractions. The
//! executor consumes a flattened, sequenced IR (the same shape
//! `wasmtime_environ::component::Component` carries) and drives
//! `wasm_runtime_layer::Module::new` and
//! `wasm_runtime_layer::Instance::new` per `(core module ...)` and
//! `(core instance N (instantiate $module ...))` directive. Compiling
//! a core module is awaited, because the browser compiles large
//! modules only through its asynchronous API; instantiating and
//! calling complete without suspending on both targets today.
//!
//! IR construction is shared across targets: `wasmtime_environ`'s
//! component `Translator` runs on every supported target, including
//! `wasm32-unknown-unknown`. The crate's `compile` feature, despite
//! its name, does not pull in Cranelift codegen — it only enables
//! `wasm-encoder`, `wasmprinter`, and the `gimli`/`object` write
//! paths, all of which build on `wasm32-unknown-unknown`.
//!
//! [`wasm_runtime_layer`]: https://docs.rs/wasm_runtime_layer

mod async_start_call;
mod callback_task;
mod cancel;
mod compile_module;
mod copy;
mod end_builtins;
mod host_copy;
mod instantiate;
mod prepare_call;
mod resource_destructor;
mod start_call;
mod start_failure;
mod start_task;
mod sync_start_call;
mod task_return;
mod thread_yield;
mod translate;

pub mod intrinsics;
pub mod ir;
pub mod trampoline;
pub mod waitable_builtins;

pub use async_start_call::build_async_start_call;
pub use callback_task::{CallbackTask, status_word};
pub use cancel::{build_subtask_cancel, build_task_cancel};
pub use compile_module::{compile_module, compile_modules};
pub use copy::{build_cancel_copy, build_copy};
pub use end_builtins::{build_drop_end, build_future_new, build_stream_new};
pub use instantiate::instantiate;
pub use prepare_call::build_prepare_call;
pub use resource_destructor::ResourceDestructor;
pub use start_call::release_subtask;
pub use sync_start_call::build_sync_start_call;
pub use task_return::build_task_return;
pub use thread_yield::build_thread_yield;
pub use translate::translate;
