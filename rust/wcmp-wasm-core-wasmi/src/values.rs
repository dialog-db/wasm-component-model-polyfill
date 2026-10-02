// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values of the runtime layer and Wasmi's, each way.
//!
//! Wasmi's values are Wasm 2.0's: numbers, vectors, `funcref`, and
//! `externref`. A reference that crosses to the host takes a slot in the
//! store, which keeps it for the life of the store.

use wcmp_wasm_core::{Error, Result, Val};

use crate::state::State;

/// The value of the runtime layer for Wasmi's `value`, whose references
/// `state` keeps from now on.
pub fn from_wasmi(state: &mut State, value: &wasmi::Val) -> Val {
    match value {
        wasmi::Val::I32(value) => Val::I32(*value),
        wasmi::Val::I64(value) => Val::I64(*value),
        wasmi::Val::F32(value) => Val::F32(value.to_bits()),
        wasmi::Val::F64(value) => Val::F64(value.to_bits()),
        wasmi::Val::V128(value) => Val::V128(value.as_u128()),
        wasmi::Val::FuncRef(func) => Val::FuncRef(nullable(*func).map(|func| state.add_func(func))),
        wasmi::Val::ExternRef(extern_ref) => {
            Val::ExternRef(nullable(*extern_ref).map(|extern_ref| state.add_extern_ref(extern_ref)))
        }
    }
}

/// The value of the runtime layer for Wasmi's reference `value`, which
/// `state` keeps from now on.
pub fn from_wasmi_ref(state: &mut State, value: &wasmi::Ref) -> Val {
    match value {
        wasmi::Ref::Func(func) => from_wasmi(state, &wasmi::Val::FuncRef(*func)),
        wasmi::Ref::Extern(extern_ref) => from_wasmi(state, &wasmi::Val::ExternRef(*extern_ref)),
    }
}

/// Wasmi's value for `value`, where the value is given for a slot of type
/// `ty`: an argument, a result of a host function, or the value of a
/// global. A value that does not match `ty` is [`Error::TypeMismatch`].
pub fn to_wasmi(state: &State, value: &Val, ty: wasmi::ValType) -> Result<wasmi::Val> {
    Ok(match (value, ty) {
        (Val::I32(value), wasmi::ValType::I32) => wasmi::Val::I32(*value),
        (Val::I64(value), wasmi::ValType::I64) => wasmi::Val::I64(*value),
        (Val::F32(bits), wasmi::ValType::F32) => wasmi::Val::F32(wasmi::F32::from_bits(*bits)),
        (Val::F64(bits), wasmi::ValType::F64) => wasmi::Val::F64(wasmi::F64::from_bits(*bits)),
        (Val::V128(value), wasmi::ValType::V128) => wasmi::Val::V128(wasmi::V128::from(*value)),
        (Val::FuncRef(func), wasmi::ValType::FuncRef) => wasmi::Val::FuncRef(match func {
            Some(func) => wasmi::Nullable::Val(*state.func(*func)?),
            None => wasmi::Nullable::Null,
        }),
        (Val::ExternRef(extern_ref), wasmi::ValType::ExternRef) => {
            wasmi::Val::ExternRef(match extern_ref {
                Some(extern_ref) => wasmi::Nullable::Val(*state.extern_ref(*extern_ref)?),
                None => wasmi::Nullable::Null,
            })
        }
        _ => {
            return Err(Error::TypeMismatch {
                message: format!("{value:?} is not a value of type `{ty:?}`"),
            });
        }
    })
}

/// Wasmi's reference for `value`, where the value is given for an element
/// of a table of `ty`. See [`to_wasmi`].
pub fn to_wasmi_ref(state: &State, value: &Val, ty: wasmi::RefType) -> Result<wasmi::Ref> {
    let ty = match ty {
        wasmi::RefType::Func => wasmi::ValType::FuncRef,
        wasmi::RefType::Extern => wasmi::ValType::ExternRef,
    };
    Ok(match to_wasmi(state, value, ty)? {
        wasmi::Val::FuncRef(func) => wasmi::Ref::Func(func),
        wasmi::Val::ExternRef(extern_ref) => wasmi::Ref::Extern(extern_ref),
        // `to_wasmi` gives a value of the reference type it was asked for.
        other => {
            return Err(Error::TypeMismatch {
                message: format!("{other:?} is not a reference"),
            });
        }
    })
}

/// The reference that `value` holds, or `None` where it is null.
fn nullable<T>(value: wasmi::Nullable<T>) -> Option<T> {
    value.into()
}
