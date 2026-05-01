//! The polyfill's owner of a successfully linked, instantiated
//! component.
//!
//! [`Instance`] exposes an export-lookup accessor — given a name and
//! the polyfill's [`Func`] type — and a calling convention that
//! drives the canonical-ABI round-trip of an exported component
//! function. Today that round-trip supports primitive valtypes only;
//! every primitive's lift and lower is direct passthrough through
//! the underlying core function and does not touch component memory.
//! Compound-valtype lift/lower lands additively in later work.

mod export_instance;
mod exports;
mod func;
#[allow(clippy::module_inception)]
mod instance;
mod typed_func;

pub use export_instance::ExportInstance;
pub use exports::InstanceExports;
pub use func::Func;
pub use instance::{ExportedFunction, Instance};
pub use typed_func::TypedFunc;
