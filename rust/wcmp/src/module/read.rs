//! Reading the imports and exports of a core module binary.
//!
//! The runtime layer describes a module's imports and exports too,
//! and the polyfill describes them to the host with its own types,
//! which express fewer than the runtime layer. The host reads a
//! module's imports and exports through [`Module::imports`] and
//! [`Module::exports`], in declaration order, so the polyfill
//! reads both lists from the binary itself. The executor pairs the
//! imports of a component's module through the runtime layer's own
//! list, which describes every type.
//!
//! [`Module::imports`]: super::Module::imports
//! [`Module::exports`]: super::Module::exports

use wasmtime_environ::wasmparser::{
    CompositeInnerType, ExternalKind, FuncType, GlobalType, MemoryType, Parser, Payload, RefType,
    TableType, TagType, TypeRef, ValType,
};

use crate::error::{Error, Result};
use crate::internal::ErrorInternal;

use super::core_extern_type::CoreExternType;
use super::core_value_type::CoreValueType;
use super::module_export::ModuleExport;
use super::module_import::ModuleImport;

/// The declared imports and exports of a validated core module, in
/// declaration order.
pub struct ModuleShape {
    /// The imports, in declaration order, but for any whose type the
    /// host's description cannot express.
    pub imports: Vec<ModuleImport>,
    /// Whether an import was left out of `imports` because the host's
    /// description cannot express its type. The host cannot supply
    /// such an import, so it cannot instantiate the module itself.
    pub undescribed_imports: bool,
    /// The exports, in declaration order, but for any whose type the
    /// host's description cannot express.
    pub exports: Vec<ModuleExport>,
    /// Whether the module declares a `start` function, which its
    /// instantiation runs.
    pub start: bool,
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
    let mut undescribed_imports = false;
    let mut raw_exports = Vec::new();
    let mut start = false;
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
                        match read_import(&mut spaces, &import)? {
                            Some(import) => imports.push(import),
                            None => undescribed_imports = true,
                        }
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
            Payload::StartSection { .. } => start = true,
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
            ExternalKind::Table => table_type(spaces.tables.get(index).ok_or_else(out_of_range)?),
            ExternalKind::Memory => Some(memory_type(
                spaces.memories.get(index).ok_or_else(out_of_range)?,
            )),
            ExternalKind::Global => {
                global_type(spaces.globals.get(index).ok_or_else(out_of_range)?)
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
        // An export whose type the host's description cannot express
        // is left out of it. Nothing of the host's reaches it, and a
        // component that aliases it reaches it through the runtime
        // layer, which describes every type.
        if let Some(ty) = ty {
            exports.push(ModuleExport {
                name: export.name.to_owned(),
                ty,
            });
        }
    }
    Ok(ModuleShape {
        imports,
        undescribed_imports,
        exports,
        start,
    })
}

/// Record one import in the index spaces and project its type, or
/// `None` where the host's description cannot express its type.
fn read_import(
    spaces: &mut IndexSpaces,
    import: &wasmtime_environ::wasmparser::Import<'_>,
) -> Result<Option<ModuleImport>> {
    let ty = match import.ty {
        TypeRef::Func(index) => {
            spaces.funcs.push(index);
            spaces.func_type(index)?
        }
        TypeRef::Table(table) => {
            spaces.tables.push(table);
            table_type(&table)
        }
        TypeRef::Memory(memory) => {
            spaces.memories.push(memory);
            Some(memory_type(&memory))
        }
        TypeRef::Global(global) => {
            spaces.globals.push(global);
            global_type(&global)
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
    Ok(ty.map(|ty| ModuleImport {
        module: import.module.to_owned(),
        name: import.name.to_owned(),
        ty,
    }))
}

impl IndexSpaces {
    /// The type of a function of the type at `type_index`, or `None`
    /// where the host's description cannot express one of its
    /// parameters or results.
    fn func_type(&self, type_index: u32) -> Result<Option<CoreExternType>> {
        let func = self
            .types
            .get(type_index as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                Error::internal("a core function names a type that is not a function type")
            })?;
        let params = func
            .params()
            .iter()
            .map(value_type)
            .collect::<Option<Vec<_>>>();
        let results = func
            .results()
            .iter()
            .map(value_type)
            .collect::<Option<Vec<_>>>();
        Ok(params
            .zip(results)
            .map(|(params, results)| CoreExternType::Func { params, results }))
    }

    /// The type of `tag`, or `None` where the host's description
    /// cannot express one of its parameters.
    fn tag_type(&self, tag: &TagType) -> Result<Option<CoreExternType>> {
        match self.func_type(tag.func_type_idx)? {
            Some(CoreExternType::Func { params, .. }) => Ok(Some(CoreExternType::Tag { params })),
            Some(_) => Err(Error::internal(
                "a tag's function type projected to a non-function",
            )),
            None => Ok(None),
        }
    }
}

fn table_type(table: &TableType) -> Option<CoreExternType> {
    Some(CoreExternType::Table {
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

fn global_type(global: &GlobalType) -> Option<CoreExternType> {
    Some(CoreExternType::Global {
        content: value_type(&global.content_type)?,
        mutable: global.mutable,
    })
}

/// The host's description of `ty`, or `None` where it has none: a
/// reference type other than `funcref` and `externref`.
fn value_type(ty: &ValType) -> Option<CoreValueType> {
    Some(match ty {
        ValType::I32 => CoreValueType::I32,
        ValType::I64 => CoreValueType::I64,
        ValType::F32 => CoreValueType::F32,
        ValType::F64 => CoreValueType::F64,
        ValType::V128 => CoreValueType::V128,
        ValType::Ref(reference) => ref_type(reference)?,
    })
}

/// The host's description of `reference`, which is `funcref` or
/// `externref`, or `None` for any other reference type.
fn ref_type(reference: &RefType) -> Option<CoreValueType> {
    if !reference.is_nullable() {
        return None;
    }
    if reference.is_func_ref() {
        Some(CoreValueType::FuncRef)
    } else if reference.is_extern_ref() {
        Some(CoreValueType::ExternRef)
    } else {
        None
    }
}

fn parse_error(err: wasmtime_environ::wasmparser::BinaryReaderError) -> Error {
    Error::internal(format!("a validated core module did not parse: {err}"))
}

fn out_of_range() -> Error {
    Error::internal("a core module export names an index outside its index space")
}
