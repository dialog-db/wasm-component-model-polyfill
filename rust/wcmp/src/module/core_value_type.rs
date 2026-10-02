// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of one core WebAssembly value.

use crate::error::{Error, Result};
use crate::internal::{CoreValueTypeInternal, ErrorInternal};

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

impl CoreValueTypeInternal for CoreValueType {
    fn from_translator(ty: &wasmtime_environ::WasmValType) -> Result<CoreValueType> {
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
