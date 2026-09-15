//! One export a core module declares.

use super::core_extern_type::CoreExternType;

/// One export of a core module: the name the module publishes the
/// item under and the item's type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModuleExport {
    /// The name the item is exported under.
    pub name: String,
    /// The type of the exported item.
    pub ty: CoreExternType,
}
