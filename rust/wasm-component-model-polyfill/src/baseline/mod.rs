//! The baseline tests that read the store's internal surface.
//!
//! A baseline test drives the polyfill the way a host does and then
//! checks what the store holds: the handle tables, the scheduler's
//! queues, the task and subtask records. That surface is the crate's
//! own, reachable through `StoreInternal` and `StoreContextInternal`
//! and through nothing a host can name, so these tests live inside
//! the crate as unit tests rather than beside it in `tests/`. Both
//! lanes run them, and the web lane runs them in a browser.
//!
//! Every other baseline test drives the polyfill through its public
//! API alone and stays in `tests/`.

mod async_start_call;
mod call_concurrent;
mod callback_export;
mod canonical_abi;
mod compile_modules;
mod destructor_task;
mod host_end_lifecycle;
mod instantiation;
mod nested_start;
mod prepared_call;
mod realloc_task;
mod resources;
mod run_concurrent;
mod stackful_export;
mod stream_copy_handles;
mod stream_copy_paths;
mod stream_future_ends;
mod stream_future_transfer;
mod subtask_drop;
mod suspend;
mod switch_module;
mod sync_lower;
mod tasks;
mod thread_builtins;
mod thread_yield;
mod waitable_builtins;
mod waitables;
