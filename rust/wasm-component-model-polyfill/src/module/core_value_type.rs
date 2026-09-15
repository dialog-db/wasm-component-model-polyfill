//! The type of one core WebAssembly value.

use wasm_runtime_layer::{RefType, ValType as RuntimeValType};

use crate::error::{Error, Result};

/// The type of a core WebAssembly value: a number, a vector, or a
/// reference. These are the value types a core module's function
/// signatures, globals, and tables name; component-level values are
/// described by [`ValueType`] instead.
///
/// [`ValueType`]: crate::ValueType
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CoreValueType {
    /// A 32-bit integer.
    I32,
    /// A 64-bit integer.
    I64,
    /// A 32-bit float.
    F32,
    /// A 64-bit float.
    F64,
    /// A 128-bit vector.
    V128,
    /// A nullable reference to a function.
    FuncRef,
    /// A nullable reference to a host value.
    ExternRef,
}

impl CoreValueType {
    /// Project a runtime-layer value type.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn from_runtime(ty: RuntimeValType) -> Self {
        match ty {
            RuntimeValType::I32 => Self::I32,
            RuntimeValType::I64 => Self::I64,
            RuntimeValType::F32 => Self::F32,
            RuntimeValType::F64 => Self::F64,
            RuntimeValType::V128 => Self::V128,
            RuntimeValType::FuncRef => Self::FuncRef,
            RuntimeValType::ExternRef => Self::ExternRef,
        }
    }

    /// Project a runtime-layer reference type, the element type of a
    /// table.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn from_runtime_ref(ty: RefType) -> Self {
        match ty {
            RefType::FuncRef => Self::FuncRef,
            RefType::ExternRef => Self::ExternRef,
        }
    }

    /// Project a translator value type. The polyfill's core surface
    /// names the reference types the runtime layer carries; a typed
    /// or non-nullable reference is reported as unsupported.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn from_translator(ty: &wasmtime_environ::WasmValType) -> Result<Self> {
        use wasmtime_environ::{WasmHeapType, WasmValType};
        Ok(match ty {
            WasmValType::I32 => Self::I32,
            WasmValType::I64 => Self::I64,
            WasmValType::F32 => Self::F32,
            WasmValType::F64 => Self::F64,
            WasmValType::V128 => Self::V128,
            WasmValType::Ref(reference) if reference.nullable => match reference.heap_type {
                WasmHeapType::Func => Self::FuncRef,
                WasmHeapType::Extern => Self::ExternRef,
                _ => {
                    return Err(Error::unsupported(
                        "garbage-collection reference types in core module types",
                    ));
                }
            },
            WasmValType::Ref(_) => {
                return Err(Error::unsupported(
                    "non-nullable reference types in core module types",
                ));
            }
        })
    }
}
