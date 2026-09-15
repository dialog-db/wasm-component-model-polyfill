//! One import a core module declares.

use super::core_extern_type::CoreExternType;

/// One import of a core module: the two-level name the module asks
/// for and the type the supplied item must have.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModuleImport {
    /// The first-level name, the module namespace of the import.
    pub module: String,
    /// The second-level name, the item inside the namespace.
    pub name: String,
    /// The type the supplied item must have.
    pub ty: CoreExternType,
}
