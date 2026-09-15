//! The polyfill's owner of a successfully linked, instantiated
//! component.
//!
//! [`Instance`] exposes an export-lookup accessor — given a name it
//! returns the polyfill's [`Func`] — and a calling convention that
//! drives the canonical-ABI round-trip of an exported component
//! function for every valtype in the synchronous baseline. Every
//! handle an instance hands out remembers the store the instance was
//! created in and refuses a call through any other store.

mod export_instance;
mod export_lookup;
mod exports;
mod func;
#[allow(clippy::module_inception)]
mod instance;
mod typed_func;

pub use export_instance::ExportInstance;
pub use export_lookup::ExportLookup;
pub use exports::InstanceExports;
pub use func::Func;
pub use instance::{ExportedFunction, Instance};
pub use typed_func::TypedFunc;
