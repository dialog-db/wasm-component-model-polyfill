//! The type of an extern.

use crate::types::{FuncType, GlobalType, MemoryType, TableType, TagType};

/// The type of an import or an export of a module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExternType {
    /// A function.
    Func(FuncType),
    /// A global.
    Global(GlobalType),
    /// A table.
    Table(TableType),
    /// A memory.
    Memory(MemoryType),
    /// A tag.
    Tag(TagType),
}
