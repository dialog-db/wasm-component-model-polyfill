//! The types of the runtime layer and Wasmtime's, each way.
//!
//! A description of Wasmtime's never fails: the runtime layer names every
//! type of Wasm 3.0. The way back fails only for a concrete type the
//! backend never described, or a limit Wasmtime refuses.

use wcmp_wasm_core::{
    Error, ExternType, FuncType, GlobalType, HeapType, MemoryType, Mutability, RefType, Result,
    TableType, TagType, ValType,
};

use crate::type_registry::TypeRegistry;

/// The runtime layer's description of the value type `ty`.
pub fn val_type(types: &TypeRegistry, ty: &wasmtime::ValType) -> ValType {
    match ty {
        wasmtime::ValType::I32 => ValType::I32,
        wasmtime::ValType::I64 => ValType::I64,
        wasmtime::ValType::F32 => ValType::F32,
        wasmtime::ValType::F64 => ValType::F64,
        wasmtime::ValType::V128 => ValType::V128,
        wasmtime::ValType::Ref(ty) => ValType::Ref(ref_type(types, ty)),
    }
}

/// The runtime layer's description of the reference type `ty`.
pub fn ref_type(types: &TypeRegistry, ty: &wasmtime::RefType) -> RefType {
    RefType::new(ty.is_nullable(), heap_type(types, ty.heap_type()))
}

/// The runtime layer's description of the heap type `ty`.
pub fn heap_type(types: &TypeRegistry, ty: &wasmtime::HeapType) -> HeapType {
    match ty {
        wasmtime::HeapType::Extern => HeapType::Extern,
        wasmtime::HeapType::NoExtern => HeapType::NoExtern,
        wasmtime::HeapType::Func => HeapType::Func,
        wasmtime::HeapType::NoFunc => HeapType::NoFunc,
        wasmtime::HeapType::Any => HeapType::Any,
        wasmtime::HeapType::Eq => HeapType::Eq,
        wasmtime::HeapType::I31 => HeapType::I31,
        wasmtime::HeapType::Array => HeapType::Array,
        wasmtime::HeapType::Struct => HeapType::Struct,
        wasmtime::HeapType::None => HeapType::None,
        wasmtime::HeapType::Exn => HeapType::Exn,
        wasmtime::HeapType::NoExn => HeapType::NoExn,
        wasmtime::HeapType::Cont => HeapType::Cont,
        wasmtime::HeapType::NoCont => HeapType::NoCont,
        wasmtime::HeapType::ConcreteFunc(_)
        | wasmtime::HeapType::ConcreteArray(_)
        | wasmtime::HeapType::ConcreteStruct(_)
        | wasmtime::HeapType::ConcreteExn(_)
        | wasmtime::HeapType::ConcreteCont(_) => HeapType::Concrete(types.handle(ty)),
    }
}

/// The runtime layer's description of the function type `ty`.
pub fn func_type(types: &TypeRegistry, ty: &wasmtime::FuncType) -> FuncType {
    FuncType::new(
        ty.params().map(|ty| val_type(types, &ty)),
        ty.results().map(|ty| val_type(types, &ty)),
    )
}

/// The runtime layer's description of the global type `ty`.
pub fn global_type(types: &TypeRegistry, ty: &wasmtime::GlobalType) -> GlobalType {
    let mutability = match ty.mutability() {
        wasmtime::Mutability::Const => Mutability::Const,
        wasmtime::Mutability::Var => Mutability::Var,
    };
    GlobalType::new(val_type(types, ty.content()), mutability)
}

/// The runtime layer's description of the table type `ty`.
pub fn table_type(types: &TypeRegistry, ty: &wasmtime::TableType) -> TableType {
    let element = ref_type(types, ty.element());
    if ty.is_64() {
        TableType::new64(element, ty.minimum(), ty.maximum())
    } else {
        // A table addressed with 32-bit numbers has 32-bit limits, so the
        // conversions only restate what Wasmtime already checked.
        TableType::new(
            element,
            u32::try_from(ty.minimum()).unwrap_or(u32::MAX),
            ty.maximum()
                .map(|maximum| u32::try_from(maximum).unwrap_or(u32::MAX)),
        )
    }
}

/// The runtime layer's description of the memory type `ty`.
pub fn memory_type(ty: &wasmtime::MemoryType) -> MemoryType {
    let (minimum, maximum) = (ty.minimum(), ty.maximum());
    // A memory addressed with 32-bit numbers has limits that fit in 32
    // bits, and a shared memory always has a maximum; the fallbacks only
    // restate what Wasmtime already checked.
    let narrow = |pages: u64| u32::try_from(pages).unwrap_or(u32::MAX);
    match (ty.is_64(), ty.is_shared()) {
        (false, false) => MemoryType::new(narrow(minimum), maximum.map(narrow)),
        (true, false) => MemoryType::new64(minimum, maximum),
        (false, true) => MemoryType::shared(narrow(minimum), narrow(maximum.unwrap_or(minimum))),
        (true, true) => MemoryType::shared64(minimum, maximum.unwrap_or(minimum)),
    }
}

/// The runtime layer's description of the tag type `ty`.
pub fn tag_type(types: &TypeRegistry, ty: &wasmtime::TagType) -> TagType {
    TagType::new(func_type(types, ty.ty()))
}

/// The runtime layer's description of the extern type `ty`.
pub fn extern_type(types: &TypeRegistry, ty: &wasmtime::ExternType) -> ExternType {
    match ty {
        wasmtime::ExternType::Func(ty) => ExternType::Func(func_type(types, ty)),
        wasmtime::ExternType::Global(ty) => ExternType::Global(global_type(types, ty)),
        wasmtime::ExternType::Table(ty) => ExternType::Table(table_type(types, ty)),
        wasmtime::ExternType::Memory(ty) => ExternType::Memory(memory_type(ty)),
        wasmtime::ExternType::Tag(ty) => ExternType::Tag(tag_type(types, ty)),
    }
}

/// Wasmtime's value type for `ty`.
pub fn to_val_type(types: &TypeRegistry, ty: &ValType) -> Result<wasmtime::ValType> {
    Ok(match ty {
        ValType::I32 => wasmtime::ValType::I32,
        ValType::I64 => wasmtime::ValType::I64,
        ValType::F32 => wasmtime::ValType::F32,
        ValType::F64 => wasmtime::ValType::F64,
        ValType::V128 => wasmtime::ValType::V128,
        ValType::Ref(ty) => wasmtime::ValType::Ref(to_ref_type(types, ty)?),
    })
}

/// Wasmtime's reference type for `ty`.
pub fn to_ref_type(types: &TypeRegistry, ty: &RefType) -> Result<wasmtime::RefType> {
    Ok(wasmtime::RefType::new(
        ty.nullable,
        to_heap_type(types, &ty.heap)?,
    ))
}

/// Wasmtime's heap type for `ty`.
pub fn to_heap_type(types: &TypeRegistry, ty: &HeapType) -> Result<wasmtime::HeapType> {
    Ok(match ty {
        HeapType::Func => wasmtime::HeapType::Func,
        HeapType::Extern => wasmtime::HeapType::Extern,
        HeapType::Any => wasmtime::HeapType::Any,
        HeapType::Eq => wasmtime::HeapType::Eq,
        HeapType::I31 => wasmtime::HeapType::I31,
        HeapType::Struct => wasmtime::HeapType::Struct,
        HeapType::Array => wasmtime::HeapType::Array,
        HeapType::Exn => wasmtime::HeapType::Exn,
        HeapType::Cont => wasmtime::HeapType::Cont,
        HeapType::NoFunc => wasmtime::HeapType::NoFunc,
        HeapType::NoExtern => wasmtime::HeapType::NoExtern,
        HeapType::None => wasmtime::HeapType::None,
        HeapType::NoExn => wasmtime::HeapType::NoExn,
        HeapType::NoCont => wasmtime::HeapType::NoCont,
        HeapType::Concrete(handle) => types.get(*handle)?,
    })
}

/// Wasmtime's function type for `ty`, in `engine`.
pub fn to_func_type(
    engine: &wasmtime::Engine,
    types: &TypeRegistry,
    ty: &FuncType,
) -> Result<wasmtime::FuncType> {
    let params = ty
        .params()
        .iter()
        .map(|ty| to_val_type(types, ty))
        .collect::<Result<Vec<_>>>()?;
    let results = ty
        .results()
        .iter()
        .map(|ty| to_val_type(types, ty))
        .collect::<Result<Vec<_>>>()?;
    Ok(wasmtime::FuncType::new(engine, params, results))
}

/// Wasmtime's global type for `ty`.
pub fn to_global_type(types: &TypeRegistry, ty: &GlobalType) -> Result<wasmtime::GlobalType> {
    let mutability = match ty.mutability() {
        Mutability::Const => wasmtime::Mutability::Const,
        Mutability::Var => wasmtime::Mutability::Var,
    };
    Ok(wasmtime::GlobalType::new(
        to_val_type(types, ty.content())?,
        mutability,
    ))
}

/// Wasmtime's table type for `ty`.
pub fn to_table_type(types: &TypeRegistry, ty: &TableType) -> Result<wasmtime::TableType> {
    let element = to_ref_type(types, ty.element())?;
    if ty.is_64() {
        return Ok(wasmtime::TableType::new64(
            element,
            ty.minimum(),
            ty.maximum(),
        ));
    }
    // `TableType::new` stores the limits of a 32-bit table as `u32`, so
    // both fit.
    let narrow = |elements: u64| {
        u32::try_from(elements).map_err(|_| Error::TypeMismatch {
            message: format!("a 32-bit table cannot hold {elements} elements"),
        })
    };
    Ok(wasmtime::TableType::new(
        element,
        narrow(ty.minimum())?,
        ty.maximum().map(narrow).transpose()?,
    ))
}

/// Wasmtime's memory type for `ty`.
pub fn to_memory_type(ty: &MemoryType) -> Result<wasmtime::MemoryType> {
    wasmtime::MemoryTypeBuilder::new()
        .min(ty.minimum())
        .max(ty.maximum())
        .memory64(ty.is_64())
        .shared(ty.is_shared())
        .build()
        .map_err(|error| Error::TypeMismatch {
            message: format!("{error:#}"),
        })
}
