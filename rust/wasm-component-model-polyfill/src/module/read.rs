//! Reading the imports and exports of a core module binary.
//!
//! The runtime layer describes a module's imports and exports too,
//! but the browser backend enumerates them from a hash map, so their
//! order differs between targets. The executor pairs a module's
//! imports positionally with the values a component supplies, and
//! [`Module::imports`] promises declaration order, so the polyfill
//! reads both lists from the binary itself.
//!
//! [`Module::imports`]: super::Module::imports

use wasmtime_environ::wasmparser::{
    CompositeInnerType, ExternalKind, FuncType, GlobalType, MemoryType, Parser, Payload, RefType,
    TableType, TagType, TypeRef, ValType,
};

use crate::error::{Error, Result};

use super::core_extern_type::CoreExternType;
use super::core_value_type::CoreValueType;
use super::module_export::ModuleExport;
use super::module_import::ModuleImport;

/// The declared imports and exports of a validated core module, in
/// declaration order.
pub struct ModuleShape {
    /// The imports, in declaration order.
    pub imports: Vec<ModuleImport>,
    /// The exports, in declaration order.
    pub exports: Vec<ModuleExport>,
}

/// The index spaces a module's export section refers into.
#[derive(Default)]
struct IndexSpaces {
    /// Every type in the type section; `None` for a non-function
    /// type, which no import or export of the polyfill's surface
    /// names.
    types: Vec<Option<FuncType>>,
    /// The function index space, as type indices.
    funcs: Vec<u32>,
    tables: Vec<TableType>,
    memories: Vec<MemoryType>,
    globals: Vec<GlobalType>,
    tags: Vec<TagType>,
}

/// Read the shape of `bytes`, a core module the runtime layer has
/// already validated. A failure here is a polyfill invariant
/// violation, not a property of the caller's module.
pub fn read_shape(bytes: &[u8]) -> Result<ModuleShape> {
    let mut spaces = IndexSpaces::default();
    let mut imports = Vec::new();
    let mut raw_exports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(parse_error)? {
            Payload::TypeSection(reader) => {
                for group in reader {
                    for sub in group.map_err(parse_error)?.into_types() {
                        spaces.types.push(match sub.composite_type.inner {
                            CompositeInnerType::Func(func) => Some(func),
                            _ => None,
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for group in reader {
                    for entry in group.map_err(parse_error)? {
                        let (_, import) = entry.map_err(parse_error)?;
                        imports.push(read_import(&mut spaces, &import)?);
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for index in reader {
                    spaces.funcs.push(index.map_err(parse_error)?);
                }
            }
            Payload::TableSection(reader) => {
                for table in reader {
                    spaces.tables.push(table.map_err(parse_error)?.ty);
                }
            }
            Payload::MemorySection(reader) => {
                for memory in reader {
                    spaces.memories.push(memory.map_err(parse_error)?);
                }
            }
            Payload::GlobalSection(reader) => {
                for global in reader {
                    spaces.globals.push(global.map_err(parse_error)?.ty);
                }
            }
            Payload::TagSection(reader) => {
                for tag in reader {
                    spaces.tags.push(tag.map_err(parse_error)?);
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    raw_exports.push(export.map_err(parse_error)?);
                }
            }
            _ => {}
        }
    }
    let mut exports = Vec::with_capacity(raw_exports.len());
    for export in raw_exports {
        let index = export.index as usize;
        let ty = match export.kind {
            ExternalKind::Func => {
                let type_index = *spaces.funcs.get(index).ok_or_else(out_of_range)?;
                spaces.func_type(type_index)?
            }
            ExternalKind::Table => table_type(spaces.tables.get(index).ok_or_else(out_of_range)?)?,
            ExternalKind::Memory => {
                memory_type(spaces.memories.get(index).ok_or_else(out_of_range)?)
            }
            ExternalKind::Global => {
                global_type(spaces.globals.get(index).ok_or_else(out_of_range)?)?
            }
            ExternalKind::Tag => {
                spaces.tag_type(spaces.tags.get(index).ok_or_else(out_of_range)?)?
            }
            _ => {
                return Err(Error::unsupported(
                    "core module exports of a kind other than func, table, memory, global, or tag",
                ));
            }
        };
        exports.push(ModuleExport {
            name: export.name.to_owned(),
            ty,
        });
    }
    Ok(ModuleShape { imports, exports })
}

/// Record one import in the index spaces and project its type.
fn read_import(
    spaces: &mut IndexSpaces,
    import: &wasmtime_environ::wasmparser::Import<'_>,
) -> Result<ModuleImport> {
    let ty = match import.ty {
        TypeRef::Func(index) => {
            spaces.funcs.push(index);
            spaces.func_type(index)?
        }
        TypeRef::Table(table) => {
            spaces.tables.push(table);
            table_type(&table)?
        }
        TypeRef::Memory(memory) => {
            spaces.memories.push(memory);
            memory_type(&memory)
        }
        TypeRef::Global(global) => {
            spaces.globals.push(global);
            global_type(&global)?
        }
        TypeRef::Tag(tag) => {
            spaces.tags.push(tag);
            spaces.tag_type(&tag)?
        }
        _ => {
            return Err(Error::unsupported(
                "core module imports of a kind other than func, table, memory, global, or tag",
            ));
        }
    };
    Ok(ModuleImport {
        module: import.module.to_owned(),
        name: import.name.to_owned(),
        ty,
    })
}

impl IndexSpaces {
    fn func_type(&self, type_index: u32) -> Result<CoreExternType> {
        let func = self
            .types
            .get(type_index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                Error::internal("a core function names a type that is not a function type")
            })?;
        Ok(CoreExternType::Func {
            params: func
                .params()
                .iter()
                .map(value_type)
                .collect::<Result<Vec<_>>>()?,
            results: func
                .results()
                .iter()
                .map(value_type)
                .collect::<Result<Vec<_>>>()?,
        })
    }

    fn tag_type(&self, tag: &TagType) -> Result<CoreExternType> {
        let CoreExternType::Func { params, .. } = self.func_type(tag.func_type_idx)? else {
            return Err(Error::internal(
                "a tag's function type projected to a non-function",
            ));
        };
        Ok(CoreExternType::Tag { params })
    }
}

fn table_type(table: &TableType) -> Result<CoreExternType> {
    Ok(CoreExternType::Table {
        element: ref_type(&table.element_type)?,
        minimum: table.initial,
        maximum: table.maximum,
    })
}

fn memory_type(memory: &MemoryType) -> CoreExternType {
    CoreExternType::Memory {
        minimum_pages: memory.initial,
        maximum_pages: memory.maximum,
        memory64: memory.memory64,
        shared: memory.shared,
    }
}

fn global_type(global: &GlobalType) -> Result<CoreExternType> {
    Ok(CoreExternType::Global {
        content: value_type(&global.content_type)?,
        mutable: global.mutable,
    })
}

fn value_type(ty: &ValType) -> Result<CoreValueType> {
    Ok(match ty {
        ValType::I32 => CoreValueType::I32,
        ValType::I64 => CoreValueType::I64,
        ValType::F32 => CoreValueType::F32,
        ValType::F64 => CoreValueType::F64,
        ValType::V128 => CoreValueType::V128,
        ValType::Ref(reference) => ref_type(reference)?,
    })
}

fn ref_type(reference: &RefType) -> Result<CoreValueType> {
    if reference.is_nullable() {
        if reference.is_func_ref() {
            return Ok(CoreValueType::FuncRef);
        }
        if reference.is_extern_ref() {
            return Ok(CoreValueType::ExternRef);
        }
        return Err(Error::unsupported(
            "garbage-collection reference types in core module types",
        ));
    }
    Err(Error::unsupported(
        "non-nullable reference types in core module types",
    ))
}

fn parse_error(err: wasmtime_environ::wasmparser::BinaryReaderError) -> Error {
    Error::internal(format!("a validated core module did not parse: {err}"))
}

fn out_of_range() -> Error {
    Error::internal("a core module export names an index outside its index space")
}
