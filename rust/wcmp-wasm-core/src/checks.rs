//! The checks the engine makes before it reaches a backend.
//!
//! Each check is the same on every backend, so the engine makes it once,
//! here, and a host sees one error for one mistake whatever the backend.

use crate::capability::{Capabilities, Capability};
use crate::contract::{BackendStore, RawHandle};
use crate::error::{Error, Result};
use crate::externs::Extern;
use crate::types::{FuncType, HeapType, MemoryType, RefType, TableType, ValType};
use crate::values::Val;

/// [`Error::WrongStore`] where `handle` belongs to a store other than
/// `store`.
pub fn same_store(store: &dyn BackendStore, handle: impl RawHandle) -> Result<()> {
    if handle.store_id() == store.id() {
        Ok(())
    } else {
        Err(Error::WrongStore)
    }
}

/// [`Error::WrongStore`] where a reference among `values` belongs to a
/// store other than `store`.
pub fn values_in_store(store: &dyn BackendStore, values: &[Val]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| value_in_store(store, value))
}

/// [`Error::WrongStore`] where `value` is a reference that belongs to a
/// store other than `store`.
pub fn value_in_store(store: &dyn BackendStore, value: &Val) -> Result<()> {
    match value {
        Val::FuncRef(Some(func)) => same_store(store, *func),
        Val::ExternRef(Some(extern_ref)) => same_store(store, *extern_ref),
        Val::AnyRef(Some(any_ref)) => same_store(store, *any_ref),
        Val::ExnRef(Some(exn_ref)) => same_store(store, *exn_ref),
        Val::ContRef(Some(cont_ref)) => same_store(store, *cont_ref),
        Val::I32(_)
        | Val::I64(_)
        | Val::F32(_)
        | Val::F64(_)
        | Val::V128(_)
        | Val::FuncRef(None)
        | Val::ExternRef(None)
        | Val::AnyRef(None)
        | Val::ExnRef(None)
        | Val::ContRef(None) => Ok(()),
    }
}

/// [`Error::WrongStore`] where `external` belongs to a store other than
/// `store`.
pub fn extern_in_store(store: &dyn BackendStore, external: &Extern) -> Result<()> {
    match external {
        Extern::Func(func) => same_store(store, *func),
        Extern::Global(global) => same_store(store, *global),
        Extern::Table(table) => same_store(store, *table),
        Extern::Memory(memory) => same_store(store, *memory),
        Extern::Tag(tag) => same_store(store, *tag),
    }
}

/// [`Error::Unsupported`] where a value of `ty` needs a capability that
/// `capabilities` lacks.
///
/// A number, a vector, `funcref`, and `externref` are the floor. A
/// reference that is not nullable, or to a concrete type, needs typed
/// function references. The internal hierarchy needs garbage collection,
/// the exception hierarchy exception handling, and the continuation
/// hierarchy stack switching.
pub fn val_type(capabilities: Capabilities, ty: &ValType) -> Result<()> {
    match ty {
        ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64 | ValType::V128 => Ok(()),
        ValType::Ref(ref_type) => self::ref_type(capabilities, ref_type),
    }
}

/// [`Error::Unsupported`] where a reference of `ty` needs a capability
/// that `capabilities` lacks. See [`val_type`].
pub fn ref_type(capabilities: Capabilities, ty: &RefType) -> Result<()> {
    let hierarchy = match ty.heap {
        HeapType::Func | HeapType::Extern => None,
        HeapType::NoFunc | HeapType::NoExtern | HeapType::Concrete(_) => {
            Some(Capability::FunctionReferences)
        }
        HeapType::Any
        | HeapType::Eq
        | HeapType::I31
        | HeapType::Struct
        | HeapType::Array
        | HeapType::None => Some(Capability::Gc),
        HeapType::Exn | HeapType::NoExn => Some(Capability::Exceptions),
        HeapType::Cont | HeapType::NoCont => Some(Capability::StackSwitching),
    };
    if let Some(capability) = hierarchy {
        capabilities.require(capability)?;
    }
    if ty.nullable {
        Ok(())
    } else {
        capabilities.require(Capability::FunctionReferences)
    }
}

/// [`Error::Unsupported`] where a function of `ty` needs a capability that
/// `capabilities` lacks.
pub fn func_type(capabilities: Capabilities, ty: &FuncType) -> Result<()> {
    ty.params()
        .iter()
        .chain(ty.results())
        .try_for_each(|ty| val_type(capabilities, ty))
}

/// [`Error::Unsupported`] where a memory of `ty` needs a capability that
/// `capabilities` lacks.
pub fn memory_type(capabilities: Capabilities, ty: &MemoryType) -> Result<()> {
    if ty.is_64() {
        capabilities.require(Capability::Memory64)?;
    }
    if ty.is_shared() {
        capabilities.require(Capability::Threads)?;
    }
    Ok(())
}

/// [`Error::Unsupported`] where a table of `ty` needs a capability that
/// `capabilities` lacks.
pub fn table_type(capabilities: Capabilities, ty: &TableType) -> Result<()> {
    if ty.is_64() {
        capabilities.require(Capability::Memory64)?;
    }
    ref_type(capabilities, ty.element())
}
