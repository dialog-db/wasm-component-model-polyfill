// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The types of the runtime layer and Wasmi's, each way.
//!
//! Wasmi's type model is Wasm 2.0's, so a description of Wasmi's never
//! fails. The way back fails for a type above Wasm 2.0, with the capability
//! the type needs, and for a limit Wasmi refuses.

use wcmp_wasm_core::{
    Capability, Error, ExternType, FuncType, GlobalType, HeapType, MemoryType, Mutability, RefType,
    Result, TableType, ValType,
};

/// The runtime layer's description of the value type `ty`.
pub fn val_type(ty: wasmi::ValType) -> ValType {
    match ty {
        wasmi::ValType::I32 => ValType::I32,
        wasmi::ValType::I64 => ValType::I64,
        wasmi::ValType::F32 => ValType::F32,
        wasmi::ValType::F64 => ValType::F64,
        wasmi::ValType::V128 => ValType::V128,
        wasmi::ValType::FuncRef => ValType::FUNCREF,
        wasmi::ValType::ExternRef => ValType::EXTERNREF,
    }
}

/// The runtime layer's description of the reference type `ty`.
pub fn ref_type(ty: wasmi::RefType) -> RefType {
    match ty {
        wasmi::RefType::Func => RefType::FUNCREF,
        wasmi::RefType::Extern => RefType::EXTERNREF,
    }
}

/// The runtime layer's description of the function type `ty`.
pub fn func_type(ty: &wasmi::FuncType) -> FuncType {
    FuncType::new(
        ty.params().iter().copied().map(val_type),
        ty.results().iter().copied().map(val_type),
    )
}

/// The runtime layer's description of the global type `ty`.
pub fn global_type(ty: wasmi::GlobalType) -> GlobalType {
    let mutability = match ty.mutability() {
        wasmi::Mutability::Const => Mutability::Const,
        wasmi::Mutability::Var => Mutability::Var,
    };
    GlobalType::new(val_type(ty.content()), mutability)
}

/// The runtime layer's description of the table type `ty`.
pub fn table_type(ty: wasmi::TableType) -> TableType {
    let element = ref_type(ty.element());
    if ty.is_64() {
        TableType::new64(element, ty.minimum(), ty.maximum())
    } else {
        // A table addressed with 32-bit numbers has 32-bit limits, so the
        // conversions only restate what Wasmi already checked.
        let narrow = |elements: u64| u32::try_from(elements).unwrap_or(u32::MAX);
        TableType::new(element, narrow(ty.minimum()), ty.maximum().map(narrow))
    }
}

/// The runtime layer's description of the memory type `ty`.
pub fn memory_type(ty: wasmi::MemoryType) -> MemoryType {
    if ty.is_64() {
        MemoryType::new64(ty.minimum(), ty.maximum())
    } else {
        // A memory addressed with 32-bit numbers has limits that fit in 32
        // bits, so the conversions only restate what Wasmi already checked.
        let narrow = |pages: u64| u32::try_from(pages).unwrap_or(u32::MAX);
        MemoryType::new(narrow(ty.minimum()), ty.maximum().map(narrow))
    }
}

/// The runtime layer's description of the extern type `ty`.
pub fn extern_type(ty: &wasmi::ExternType) -> ExternType {
    match ty {
        wasmi::ExternType::Func(ty) => ExternType::Func(func_type(ty)),
        wasmi::ExternType::Global(ty) => ExternType::Global(global_type(*ty)),
        wasmi::ExternType::Table(ty) => ExternType::Table(table_type(*ty)),
        wasmi::ExternType::Memory(ty) => ExternType::Memory(memory_type(*ty)),
    }
}

/// Wasmi's value type for `ty`.
pub fn to_val_type(ty: &ValType) -> Result<wasmi::ValType> {
    Ok(match ty {
        ValType::I32 => wasmi::ValType::I32,
        ValType::I64 => wasmi::ValType::I64,
        ValType::F32 => wasmi::ValType::F32,
        ValType::F64 => wasmi::ValType::F64,
        ValType::V128 => wasmi::ValType::V128,
        ValType::Ref(ty) => match to_ref_type(ty)? {
            wasmi::RefType::Func => wasmi::ValType::FuncRef,
            wasmi::RefType::Extern => wasmi::ValType::ExternRef,
        },
    })
}

/// Wasmi's reference type for `ty`: `funcref` or `externref`.
///
/// Every other reference type needs a capability above Wasm 2.0, which the
/// backend does not declare, so it is [`Error::Unsupported`] with that
/// capability: typed function references for a reference that is not
/// nullable, or to a concrete type or a bottom type, and the capability of
/// the hierarchy for the internal, exception, and continuation references.
pub fn to_ref_type(ty: &RefType) -> Result<wasmi::RefType> {
    let capability = match ty.heap {
        HeapType::Func if ty.nullable => return Ok(wasmi::RefType::Func),
        HeapType::Extern if ty.nullable => return Ok(wasmi::RefType::Extern),
        HeapType::Any
        | HeapType::Eq
        | HeapType::I31
        | HeapType::Struct
        | HeapType::Array
        | HeapType::None => Capability::Gc,
        HeapType::Exn | HeapType::NoExn => Capability::Exceptions,
        HeapType::Cont | HeapType::NoCont => Capability::StackSwitching,
        HeapType::Func
        | HeapType::Extern
        | HeapType::NoFunc
        | HeapType::NoExtern
        | HeapType::Concrete(_) => Capability::FunctionReferences,
    };
    Err(Error::Unsupported(capability))
}

/// The most parameters, and the most results, a function type of Wasmi's
/// has.
const MAX_FUNC_TYPE_LEN: usize = 1_000;

/// Wasmi's function type for `ty`.
pub fn to_func_type(ty: &FuncType) -> Result<wasmi::FuncType> {
    let params = ty
        .params()
        .iter()
        .map(to_val_type)
        .collect::<Result<Vec<_>>>()?;
    let results = ty
        .results()
        .iter()
        .map(to_val_type)
        .collect::<Result<Vec<_>>>()?;
    // Wasmi panics on a type above its limit, so the backend refuses such a
    // type before it reaches Wasmi.
    if params.len() > MAX_FUNC_TYPE_LEN || results.len() > MAX_FUNC_TYPE_LEN {
        return Err(Error::TypeMismatch {
            message: format!(
                "Wasmi takes at most {MAX_FUNC_TYPE_LEN} parameters and {MAX_FUNC_TYPE_LEN} \
                 results, and the type has {} and {}",
                params.len(),
                results.len(),
            ),
        });
    }
    Ok(wasmi::FuncType::new(params, results))
}

/// Wasmi's mutability for `mutability`.
pub fn to_mutability(mutability: Mutability) -> wasmi::Mutability {
    match mutability {
        Mutability::Const => wasmi::Mutability::Const,
        Mutability::Var => wasmi::Mutability::Var,
    }
}

/// Wasmi's table type for `ty`.
pub fn to_table_type(ty: &TableType) -> Result<wasmi::TableType> {
    let element = to_ref_type(ty.element())?;
    // Wasmi panics on a maximum below the minimum, so the backend refuses
    // such a type before it reaches Wasmi.
    if let Some(maximum) = ty.maximum()
        && maximum < ty.minimum()
    {
        return Err(Error::TypeMismatch {
            message: format!(
                "the table's maximum of {maximum} elements is below its minimum of {}",
                ty.minimum()
            ),
        });
    }
    if ty.is_64() {
        return Ok(wasmi::TableType::new64(element, ty.minimum(), ty.maximum()));
    }
    // A table addressed with 32-bit numbers has 32-bit limits.
    let narrow = |elements: u64| {
        u32::try_from(elements).map_err(|_| Error::TypeMismatch {
            message: format!("a 32-bit table cannot hold {elements} elements"),
        })
    };
    Ok(wasmi::TableType::new(
        element,
        narrow(ty.minimum())?,
        ty.maximum().map(narrow).transpose()?,
    ))
}

/// Wasmi's memory type for `ty`.
///
/// Wasmi has no shared memory, so a shared type is
/// [`Error::Unsupported`] with `threads`. A maximum that Wasmi takes, and
/// then panics on when the memory grows, is [`Error::TypeMismatch`]: see
/// [`grows_safely`].
pub fn to_memory_type(ty: &MemoryType) -> Result<wasmi::MemoryType> {
    if ty.is_shared() {
        return Err(Error::Unsupported(Capability::Threads));
    }
    let mut builder = wasmi::MemoryType::builder();
    builder
        .memory64(ty.is_64())
        .min(ty.minimum())
        .max(ty.maximum());
    let wasm_ty = builder.build().map_err(|error| Error::TypeMismatch {
        message: error.to_string(),
    })?;
    if !grows_safely(wasm_ty) {
        return Err(Error::TypeMismatch {
            message: format!(
                "Wasmi cannot grow a memory whose maximum of {} pages is 2^64 bytes or more",
                ty.maximum().unwrap_or_default()
            ),
        });
    }
    Ok(wasm_ty)
}

/// Whether Wasmi can grow a memory of the type `ty` without a panic.
///
/// Wasmi counts the maximum of a memory in bytes, in a `u64`, each time the
/// memory grows, and panics where that overflows. A 64-bit memory may have
/// a maximum of 2^48 pages, which is 2^64 bytes, so Wasmi takes such a
/// memory and then panics on every growth of it by one page or more
/// (`wasmi_core` 2.0.0, `crates/core/src/memory/mod.rs:165`). The backend
/// leaves Wasmi's custom page sizes off, so every page is 64 KiB.
pub fn grows_safely(ty: wasmi::MemoryType) -> bool {
    const PAGE_SIZE: u64 = 1 << 16;
    ty.maximum()
        .is_none_or(|maximum| maximum.checked_mul(PAGE_SIZE).is_some())
}
