//! One export of a module.

use crate::types::ExternType;

/// One export of a module: its name and its type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExportType {
    name: String,
    ty: ExternType,
}

impl ExportType {
    /// The export `name` of type `ty`.
    pub fn new(name: impl Into<String>, ty: ExternType) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }

    /// The name of the export.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The type of the export.
    pub fn ty(&self) -> &ExternType {
        &self.ty
    }
}
