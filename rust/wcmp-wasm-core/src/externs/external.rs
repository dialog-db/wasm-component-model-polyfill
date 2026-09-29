//! An import or an export of an instance.

use crate::externs::{Func, Global, Memory, Table, Tag};

/// A value an instance imports or exports: a function, a global, a table, a
/// memory, or a tag.
#[derive(Clone, Copy, Debug)]
pub enum Extern {
    /// A function.
    Func(Func),
    /// A global.
    Global(Global),
    /// A table.
    Table(Table),
    /// A memory.
    Memory(Memory),
    /// A tag.
    Tag(Tag),
}

impl Extern {
    /// The function, where the extern is one.
    pub fn into_func(self) -> Option<Func> {
        match self {
            Extern::Func(func) => Some(func),
            _ => None,
        }
    }

    /// The global, where the extern is one.
    pub fn into_global(self) -> Option<Global> {
        match self {
            Extern::Global(global) => Some(global),
            _ => None,
        }
    }

    /// The table, where the extern is one.
    pub fn into_table(self) -> Option<Table> {
        match self {
            Extern::Table(table) => Some(table),
            _ => None,
        }
    }

    /// The memory, where the extern is one.
    pub fn into_memory(self) -> Option<Memory> {
        match self {
            Extern::Memory(memory) => Some(memory),
            _ => None,
        }
    }

    /// The tag, where the extern is one.
    pub fn into_tag(self) -> Option<Tag> {
        match self {
            Extern::Tag(tag) => Some(tag),
            _ => None,
        }
    }
}

impl From<Func> for Extern {
    fn from(func: Func) -> Self {
        Extern::Func(func)
    }
}

impl From<Global> for Extern {
    fn from(global: Global) -> Self {
        Extern::Global(global)
    }
}

impl From<Table> for Extern {
    fn from(table: Table) -> Self {
        Extern::Table(table)
    }
}

impl From<Memory> for Extern {
    fn from(memory: Memory) -> Self {
        Extern::Memory(memory)
    }
}

impl From<Tag> for Extern {
    fn from(tag: Tag) -> Self {
        Extern::Tag(tag)
    }
}
