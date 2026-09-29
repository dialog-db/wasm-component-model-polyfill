//! A compiled module, and the description of its boundary.

mod export_type;
mod import_type;
#[allow(clippy::module_inception)]
mod module;

pub use export_type::ExportType;
pub use import_type::ImportType;
pub use module::Module;
