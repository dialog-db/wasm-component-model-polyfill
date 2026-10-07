// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The boundary of a module, read from its bytes.

use wasmparser::{CompositeInnerType, ExternalKind, Operator, Parser, Payload, SubType, TypeRef};
use wcmp_wasm_core::{
    Error, ExportType, ExternType, FuncType, GlobalType, HeapType, ImportType, MemoryType,
    Mutability, RefType, Result, TableType, TagType, TypeHandle, ValType,
};

use crate::type_registry::TypeRegistry;

/// The imports and the exports of a module.
///
/// The JavaScript API names the imports and exports of a module and their
/// kinds, but not their types. So the backend reads the types from the
/// bytes of the module. It reads the sections that the boundary needs, and
/// the function bodies of a module that has a memory, only to learn
/// whether one waits or notifies on a memory. It never reads a data
/// segment or an element segment.
#[derive(Debug)]
pub struct Boundary {
    /// The imports, in the order an instantiation takes them.
    pub imports: Vec<ImportType>,
    /// The exports, in the order the module declares them.
    pub exports: Vec<ExportType>,
    /// Whether a memory of the module, imported or its own, is shared.
    ///
    /// The memory section comes before the export section, so the reader
    /// sees every memory. The store reads a trap of an atomic wait by it.
    pub shared_memory: bool,
    /// Whether a function of the module waits or notifies on a memory,
    /// with `memory.atomic.wait32`, `memory.atomic.wait64`, or
    /// `memory.atomic.notify`. The store reads JavaScriptCore's message
    /// for an access out of the bounds of a memory by it, since that
    /// engine gives a refused wait or notify the same message.
    pub atomic_wait: bool,
}

/// The index spaces of a module, as far as the boundary needs them.
#[derive(Default)]
struct Spaces {
    handles: Vec<TypeHandle>,
    types: Vec<SubType>,
    funcs: Vec<u32>,
    tables: Vec<wasmparser::TableType>,
    memories: Vec<wasmparser::MemoryType>,
    globals: Vec<wasmparser::GlobalType>,
    tags: Vec<wasmparser::TagType>,
}

impl Boundary {
    /// The boundary of the module `bytes`, whose concrete types `types`
    /// numbers.
    ///
    /// The engine accepted the module before the backend reads it, so a
    /// failure here is a module the engine and the reader disagree on. It
    /// is [`Error::Compile`], with the reader's message.
    pub fn read(bytes: &[u8], types: &TypeRegistry) -> Result<Self> {
        let mut spaces = Spaces::default();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut atomic_wait = false;
        for payload in Parser::new(0).parse_all(bytes) {
            match payload.map_err(compile)? {
                Payload::TypeSection(reader) => {
                    for group in reader {
                        let group = group.map_err(compile)?;
                        let start = spaces.types.len() as u32;
                        let handles = types.intern(&group, start, &spaces.handles);
                        spaces.handles.extend(handles);
                        spaces.types.extend(group.into_types());
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.map_err(compile)?;
                        let ty = spaces.import(import.ty)?;
                        imports.push(ImportType::new(import.module, import.name, ty));
                    }
                }
                Payload::FunctionSection(reader) => {
                    for ty in reader {
                        spaces.funcs.push(ty.map_err(compile)?);
                    }
                }
                Payload::TableSection(reader) => {
                    for table in reader {
                        spaces.tables.push(table.map_err(compile)?.ty);
                    }
                }
                Payload::MemorySection(reader) => {
                    for memory in reader {
                        spaces.memories.push(memory.map_err(compile)?);
                    }
                }
                Payload::GlobalSection(reader) => {
                    for global in reader {
                        spaces.globals.push(global.map_err(compile)?.ty);
                    }
                }
                Payload::TagSection(reader) => {
                    for tag in reader {
                        spaces.tags.push(tag.map_err(compile)?);
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.map_err(compile)?;
                        let ty = spaces.export(export.kind, export.index)?;
                        exports.push(ExportType::new(export.name, ty));
                    }
                }
                // Nothing after the export section crosses the boundary.
                // A function body tells only whether it waits or
                // notifies, which a module without a memory cannot.
                Payload::CodeSectionStart { .. } if spaces.memories.is_empty() => break,
                Payload::CodeSectionEntry(body) if waits(&body)? => {
                    atomic_wait = true;
                    break;
                }
                _ => {}
            }
        }
        let shared_memory = spaces.memories.iter().any(|memory| memory.shared);
        Ok(Self {
            imports,
            exports,
            shared_memory,
            atomic_wait,
        })
    }
}

/// Whether the function `body` waits or notifies on a memory.
fn waits(body: &wasmparser::FunctionBody<'_>) -> Result<bool> {
    let mut operators = body.get_operators_reader().map_err(compile)?;
    while !operators.eof() {
        match operators.read().map_err(compile)? {
            Operator::MemoryAtomicWait32 { .. }
            | Operator::MemoryAtomicWait64 { .. }
            | Operator::MemoryAtomicNotify { .. } => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

impl Spaces {
    /// The type of an import of type `ty`, which the import adds to its
    /// index space.
    fn import(&mut self, ty: TypeRef) -> Result<ExternType> {
        match ty {
            TypeRef::Func(index) | TypeRef::FuncExact(index) => {
                self.funcs.push(index);
                Ok(ExternType::Func(self.func_type(index)?))
            }
            TypeRef::Table(table) => {
                self.tables.push(table);
                Ok(ExternType::Table(self.table_type(table)))
            }
            TypeRef::Memory(memory) => {
                self.memories.push(memory);
                Ok(ExternType::Memory(memory_type(memory)))
            }
            TypeRef::Global(global) => {
                self.globals.push(global);
                Ok(ExternType::Global(self.global_type(global)))
            }
            TypeRef::Tag(tag) => {
                self.tags.push(tag);
                Ok(ExternType::Tag(TagType::new(
                    self.func_type(tag.func_type_idx)?,
                )))
            }
        }
    }

    /// The type of the export of kind `kind` at `index`.
    fn export(&self, kind: ExternalKind, index: u32) -> Result<ExternType> {
        let missing = || {
            compile(format!(
                "the module exports an item {index} it does not define"
            ))
        };
        let at = |len: usize| (index as usize) < len;
        match kind {
            ExternalKind::Func | ExternalKind::FuncExact => {
                let ty = *self.funcs.get(index as usize).ok_or_else(missing)?;
                Ok(ExternType::Func(self.func_type(ty)?))
            }
            ExternalKind::Table if at(self.tables.len()) => Ok(ExternType::Table(
                self.table_type(self.tables[index as usize]),
            )),
            ExternalKind::Memory if at(self.memories.len()) => Ok(ExternType::Memory(memory_type(
                self.memories[index as usize],
            ))),
            ExternalKind::Global if at(self.globals.len()) => Ok(ExternType::Global(
                self.global_type(self.globals[index as usize]),
            )),
            ExternalKind::Tag if at(self.tags.len()) => Ok(ExternType::Tag(TagType::new(
                self.func_type(self.tags[index as usize].func_type_idx)?,
            ))),
            _ => Err(missing()),
        }
    }

    /// The function type at type index `index`.
    fn func_type(&self, index: u32) -> Result<FuncType> {
        match self
            .types
            .get(index as usize)
            .map(|ty| &ty.composite_type.inner)
        {
            Some(CompositeInnerType::Func(func)) => Ok(FuncType::new(
                func.params().iter().map(|ty| self.val_type(*ty)),
                func.results().iter().map(|ty| self.val_type(*ty)),
            )),
            _ => Err(compile(format!("type {index} is not a function type"))),
        }
    }

    fn table_type(&self, ty: wasmparser::TableType) -> TableType {
        let element = self.ref_type(ty.element_type);
        if ty.table64 {
            TableType::new64(element, ty.initial, ty.maximum)
        } else {
            // A valid table addressed with 32-bit numbers has 32-bit
            // limits.
            TableType::new(
                element,
                ty.initial as u32,
                ty.maximum.map(|maximum| maximum as u32),
            )
        }
    }

    fn global_type(&self, ty: wasmparser::GlobalType) -> GlobalType {
        let mutability = if ty.mutable {
            Mutability::Var
        } else {
            Mutability::Const
        };
        GlobalType::new(self.val_type(ty.content_type), mutability)
    }

    fn val_type(&self, ty: wasmparser::ValType) -> ValType {
        match ty {
            wasmparser::ValType::I32 => ValType::I32,
            wasmparser::ValType::I64 => ValType::I64,
            wasmparser::ValType::F32 => ValType::F32,
            wasmparser::ValType::F64 => ValType::F64,
            wasmparser::ValType::V128 => ValType::V128,
            wasmparser::ValType::Ref(ty) => ValType::Ref(self.ref_type(ty)),
        }
    }

    fn ref_type(&self, ty: wasmparser::RefType) -> RefType {
        use wasmparser::AbstractHeapType as Abstract;
        let heap = match ty.heap_type() {
            // The type model has no shared references, so a shared
            // abstract type is described as its unshared twin.
            wasmparser::HeapType::Abstract { ty, .. } => match ty {
                Abstract::Func => HeapType::Func,
                Abstract::Extern => HeapType::Extern,
                Abstract::Any => HeapType::Any,
                Abstract::None => HeapType::None,
                Abstract::NoExtern => HeapType::NoExtern,
                Abstract::NoFunc => HeapType::NoFunc,
                Abstract::Eq => HeapType::Eq,
                Abstract::Struct => HeapType::Struct,
                Abstract::Array => HeapType::Array,
                Abstract::I31 => HeapType::I31,
                Abstract::Exn => HeapType::Exn,
                Abstract::NoExn => HeapType::NoExn,
                Abstract::Cont => HeapType::Cont,
                Abstract::NoCont => HeapType::NoCont,
            },
            wasmparser::HeapType::Concrete(index) | wasmparser::HeapType::Exact(index) => {
                match index
                    .as_module_index()
                    .and_then(|index| self.handles.get(index as usize))
                {
                    Some(handle) => HeapType::Concrete(*handle),
                    // Every concrete type at the boundary names a type of
                    // the module's own type section, which the backend
                    // numbered first. The bottom of the function hierarchy
                    // stands in where a module the engine accepted says
                    // otherwise.
                    None => HeapType::NoFunc,
                }
            }
        };
        RefType::new(ty.is_nullable(), heap)
    }
}

fn memory_type(ty: wasmparser::MemoryType) -> MemoryType {
    // A valid memory addressed with 32-bit numbers has 32-bit limits, and
    // a valid shared memory has a maximum.
    match (ty.memory64, ty.shared, ty.maximum) {
        (false, false, maximum) => {
            MemoryType::new(ty.initial as u32, maximum.map(|maximum| maximum as u32))
        }
        (true, false, maximum) => MemoryType::new64(ty.initial, maximum),
        (false, true, maximum) => {
            MemoryType::shared(ty.initial as u32, maximum.unwrap_or(ty.initial) as u32)
        }
        (true, true, maximum) => MemoryType::shared64(ty.initial, maximum.unwrap_or(ty.initial)),
    }
}

/// [`Error::Compile`] with the words of `error`.
fn compile(error: impl core::fmt::Display) -> Error {
    Error::Compile {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use wcmp_macros::wasm;

    use super::*;

    /// Whether the module `bytes` waits or notifies, as its boundary reads.
    fn atomic_wait(bytes: &[u8]) -> bool {
        Boundary::read(bytes, &TypeRegistry::default())
            .expect("the boundary reads")
            .atomic_wait
    }

    #[wcmp_macros::test]
    fn it_reads_whether_a_function_waits_or_notifies() {
        assert!(atomic_wait(wasm!(
            r#"
            (module
              (memory 1)
              (func (export "f") (result i32)
                i32.const 0
                i32.const 0
                i64.const 0
                memory.atomic.wait32))
            "#
        )));
        assert!(atomic_wait(wasm!(
            r#"
            (module
              (import "host" "memory" (memory 1 1 shared))
              (func)
              (func (result i32)
                i32.const 0
                i32.const 1
                memory.atomic.notify))
            "#
        )));
        assert!(!atomic_wait(wasm!(
            r#"
            (module
              (memory 1)
              (func (export "f") (result i32)
                i32.const 0
                i32.atomic.load))
            "#
        )));
        assert!(
            !atomic_wait(wasm!(r#"(module (func (export "f")))"#)),
            "a module without a memory cannot wait"
        );
    }
}
