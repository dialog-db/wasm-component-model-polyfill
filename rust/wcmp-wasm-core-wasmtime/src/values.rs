// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The values of the runtime layer and Wasmtime's, each way.
//!
//! A value that crosses to the host is kept by the store: a reference in
//! it takes a slot, rooted until the store drops. A value that crosses to a
//! guest is rooted in the current scope of the store only, which ends when
//! the operation that made it returns.

use wasmtime::{AsContext, AsContextMut, HeapTopType};
use wcmp_wasm_core::{Error, Result, Val};

use crate::errors;
use crate::state::State;

/// The value of the runtime layer for Wasmtime's `value`, whose references
/// `store` keeps from now on.
pub fn from_wasmtime(
    store: &mut impl AsContextMut<Data = State>,
    value: &wasmtime::Val,
) -> Result<Val> {
    Ok(match value {
        wasmtime::Val::I32(value) => Val::I32(*value),
        wasmtime::Val::I64(value) => Val::I64(*value),
        wasmtime::Val::F32(bits) => Val::F32(*bits),
        wasmtime::Val::F64(bits) => Val::F64(*bits),
        wasmtime::Val::V128(value) => Val::V128(value.as_u128()),
        wasmtime::Val::FuncRef(func) => {
            Val::FuncRef(func.map(|func| store.as_context_mut().data_mut().add_func(func)))
        }
        wasmtime::Val::ExternRef(None) => Val::ExternRef(None),
        wasmtime::Val::ExternRef(Some(extern_ref)) => {
            let extern_ref = extern_ref
                .to_owned_rooted(&mut *store)
                .map_err(errors::backend)?;
            Val::ExternRef(Some(
                store.as_context_mut().data_mut().add_extern_ref(extern_ref),
            ))
        }
        wasmtime::Val::AnyRef(None) => Val::AnyRef(None),
        wasmtime::Val::AnyRef(Some(any_ref)) => {
            let any_ref = any_ref
                .to_owned_rooted(&mut *store)
                .map_err(errors::backend)?;
            Val::AnyRef(Some(store.as_context_mut().data_mut().add_any_ref(any_ref)))
        }
        wasmtime::Val::ExnRef(None) => Val::ExnRef(None),
        wasmtime::Val::ExnRef(Some(exn_ref)) => {
            let exn_ref = exn_ref
                .to_owned_rooted(&mut *store)
                .map_err(errors::backend)?;
            Val::ExnRef(Some(store.as_context_mut().data_mut().add_exn_ref(exn_ref)))
        }
        wasmtime::Val::ContRef(None) => Val::ContRef(None),
        wasmtime::Val::ContRef(Some(_)) => return Err(errors::continuation()),
    })
}

/// Wasmtime's value for `value`, where the value is given for a slot of
/// type `ty`: an argument, a result of a host function, or the value of a
/// global.
///
/// A null takes the null of `ty`'s own hierarchy. The runtime layer has one
/// null for each hierarchy, and cannot tell which hierarchy a concrete type
/// belongs to; Wasmtime can. A value that does not match `ty` is
/// [`Error::TypeMismatch`].
pub fn to_wasmtime(
    store: &mut impl AsContextMut<Data = State>,
    value: &Val,
    ty: &wasmtime::ValType,
) -> Result<wasmtime::Val> {
    let converted = if let wasmtime::ValType::Ref(ref_type) = ty
        && value.is_null()
    {
        if ref_type.heap_type().top() == HeapTopType::Cont {
            return Err(errors::continuation());
        }
        wasmtime::Val::null_ref(ref_type.heap_type())
    } else {
        untyped(store, value)?
    };
    if converted
        .matches_ty(store.as_context(), ty)
        .map_err(errors::backend)?
    {
        Ok(converted)
    } else {
        Err(Error::TypeMismatch {
            message: format!("{value:?} is not a value of type `{ty}`"),
        })
    }
}

/// Wasmtime's reference for `value`, where the value is given for an
/// element of a table of `ty`. See [`to_wasmtime`].
pub fn to_wasmtime_ref(
    store: &mut impl AsContextMut<Data = State>,
    value: &Val,
    ty: &wasmtime::RefType,
) -> Result<wasmtime::Ref> {
    to_wasmtime(store, value, &wasmtime::ValType::Ref(ty.clone()))?
        .ref_()
        .ok_or_else(|| Error::TypeMismatch {
            message: format!("{value:?} is not a reference"),
        })
}

/// [`errors::continuation`] where one of `types` is a continuation
/// reference, which Wasmtime cannot move between the host and a guest.
pub fn refuse_continuations(types: impl IntoIterator<Item = wasmtime::ValType>) -> Result<()> {
    let continuation = types.into_iter().any(|ty| {
        matches!(&ty, wasmtime::ValType::Ref(ref_type)
            if ref_type.heap_type().top() == HeapTopType::Cont)
    });
    if continuation {
        Err(errors::continuation())
    } else {
        Ok(())
    }
}

/// Wasmtime's value for `value`, with the null of `value`'s own hierarchy.
fn untyped(store: &mut impl AsContextMut<Data = State>, value: &Val) -> Result<wasmtime::Val> {
    Ok(match *value {
        Val::I32(value) => wasmtime::Val::I32(value),
        Val::I64(value) => wasmtime::Val::I64(value),
        Val::F32(bits) => wasmtime::Val::F32(bits),
        Val::F64(bits) => wasmtime::Val::F64(bits),
        Val::V128(value) => wasmtime::Val::V128(value.into()),
        Val::FuncRef(None) => wasmtime::Val::FuncRef(None),
        Val::FuncRef(Some(func)) => wasmtime::Val::FuncRef(Some(*state(store).func(func)?)),
        Val::ExternRef(None) => wasmtime::Val::ExternRef(None),
        Val::ExternRef(Some(extern_ref)) => {
            let extern_ref = state(store).extern_ref(extern_ref)?.clone();
            wasmtime::Val::ExternRef(Some(extern_ref.to_rooted(&mut *store)))
        }
        Val::AnyRef(None) => wasmtime::Val::AnyRef(None),
        Val::AnyRef(Some(any_ref)) => {
            let any_ref = state(store).any_ref(any_ref)?.clone();
            wasmtime::Val::AnyRef(Some(any_ref.to_rooted(&mut *store)))
        }
        Val::ExnRef(None) => wasmtime::Val::ExnRef(None),
        Val::ExnRef(Some(exn_ref)) => {
            let exn_ref = state(store).exn_ref(exn_ref)?.clone();
            wasmtime::Val::ExnRef(Some(exn_ref.to_rooted(&mut *store)))
        }
        Val::ContRef(_) => return Err(errors::continuation()),
    })
}

/// The state of `store`.
fn state(store: &impl AsContext<Data = State>) -> &State {
    store.as_context().data()
}
