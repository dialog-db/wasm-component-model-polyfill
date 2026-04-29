//! The polyfill's parsed-component value and the data shapes it
//! exposes.
//!
//! [`Component`] is constructed from a byte slice and an [`Engine`]
//! via [`Component::new`]. Once parsed, a component exposes its
//! declared imports and exports as the polyfill's own data — no
//! upstream parser type leaks into the public API. Linking,
//! instantiation, and the canonical ABI are layered on top
//! elsewhere.
//!
//! [`Engine`]: crate::Engine

mod component_export;
mod component_import;
mod component_interface;
mod extern_type;
mod external_name;
mod function_type;
mod instance_type;
mod parse;

pub use component_export::ComponentExport;
pub use component_import::ComponentImport;
pub use component_interface::Component;
pub use extern_type::ExternType;
pub use external_name::ExternalName;
pub use function_type::{FunctionParameter, FunctionType};
pub use instance_type::{InstanceItem, InstanceType};
