//! The type of one import or export of a core module.

use wasm_runtime_layer::ExternType as RuntimeExternType;

use super::core_value_type::CoreValueType;

/// The type of a core module's import or export: a function, a
/// global, a linear memory, a table, or an exception tag. Sizes are
/// in pages for memories and in elements for tables, as the core
/// specification counts them.
///
/// Equality is structural.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum CoreExternType {
    /// A core function with the given parameter and result types.
    Func {
        /// The parameter types, in order.
        params: Vec<CoreValueType>,
        /// The result types, in order.
        results: Vec<CoreValueType>,
    },
    /// A global variable.
    Global {
        /// The type of the value the global holds.
        content: CoreValueType,
        /// Whether the global can be assigned after instantiation.
        mutable: bool,
    },
    /// A linear memory.
    Memory {
        /// The number of pages the memory starts with.
        minimum_pages: u64,
        /// The number of pages the memory can grow to, or `None`
        /// when it has no declared limit.
        maximum_pages: Option<u64>,
        /// Whether the memory is addressed with 64-bit offsets.
        memory64: bool,
        /// Whether the memory is shared between threads.
        shared: bool,
    },
    /// A table of references.
    Table {
        /// The type of the references the table holds.
        element: CoreValueType,
        /// The number of elements the table starts with.
        minimum: u64,
        /// The number of elements the table can grow to, or `None`
        /// when it has no declared limit.
        maximum: Option<u64>,
    },
    /// An exception tag with the given parameter types.
    Tag {
        /// The parameter types of the exception, in order.
        params: Vec<CoreValueType>,
    },
}

impl CoreExternType {
    /// Project a runtime-layer extern type.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn from_runtime(ty: &RuntimeExternType) -> Self {
        match ty {
            RuntimeExternType::Func(func) => Self::Func {
                params: func
                    .params()
                    .iter()
                    .map(|ty| CoreValueType::from_runtime(*ty))
                    .collect(),
                results: func
                    .results()
                    .iter()
                    .map(|ty| CoreValueType::from_runtime(*ty))
                    .collect(),
            },
            RuntimeExternType::Global(global) => Self::Global {
                content: CoreValueType::from_runtime(global.content()),
                mutable: global.mutable(),
            },
            RuntimeExternType::Memory(memory) => Self::Memory {
                minimum_pages: u64::from(memory.initial_pages()),
                maximum_pages: memory.maximum_pages().map(u64::from),
                memory64: false,
                shared: false,
            },
            RuntimeExternType::Table(table) => Self::Table {
                element: CoreValueType::from_runtime_ref(table.element()),
                minimum: u64::from(table.minimum()),
                maximum: table.maximum().map(u64::from),
            },
        }
    }
}
