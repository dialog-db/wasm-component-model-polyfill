//! A single declared export of a component.

use super::extern_type::ExternType;
use super::external_name::ExternalName;

/// One declared export of a parsed component.
///
/// An export pairs the name under which the component publishes an
/// item with the typed shape that item presents to a host.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComponentExport {
    /// The name under which this export is declared.
    pub name: ExternalName,
    /// The extern type the component publishes.
    pub ty: ExternType,
}
