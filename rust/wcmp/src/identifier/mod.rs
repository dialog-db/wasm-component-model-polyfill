//! Identifier types used to address component imports and exports.
//!
//! Component imports and exports are keyed by qualified names drawn
//! from the WIT identifier syntax — `namespace:name[@semver][/iface]`.
//! The polyfill exposes its own data types for these names so that
//! introspecting a parsed component never surfaces an upstream type
//! to a downstream consumer.
//!
//! Resolution against semver constraints (which candidate satisfies a
//! given import) is the linker's responsibility and is not modelled
//! here; this module is just the addressing surface.

mod interface_identifier;
mod package_name;
mod parse;

pub use interface_identifier::InterfaceIdentifier;
pub use package_name::PackageName;
pub use parse::IdentifierParseError;
