//! Values as the JavaScript API carries them.

use js_sys::Function;
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::{Error, HeapType, Result, Val, ValType};

use crate::objects::Objects;
use crate::type_registry::{Hierarchy, TypeRegistry};

/// The case of [`Val`] a value takes, which decides how the JavaScript API
/// carries it.
///
/// A number is a `Number`, and an `i64` a `BigInt`. A function reference is
/// the function itself. An `externref` and an internal reference are the
/// JavaScript value the guest holds: an `i31ref` is a `Number`, and a GC
/// object is an opaque object. Null is `null`. The JavaScript API carries
/// no `v128`, no `exnref`, and no continuation reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    I32,
    I64,
    F32,
    F64,
    V128,
    Func,
    Extern,
    Any,
    Exn,
    Cont,
}

impl Kind {
    /// The kind of the values of `ty`.
    pub fn of_type(ty: &ValType, types: &TypeRegistry) -> Kind {
        match ty {
            ValType::I32 => Kind::I32,
            ValType::I64 => Kind::I64,
            ValType::F32 => Kind::F32,
            ValType::F64 => Kind::F64,
            ValType::V128 => Kind::V128,
            ValType::Ref(ty) => match ty.heap {
                HeapType::Func | HeapType::NoFunc => Kind::Func,
                HeapType::Extern | HeapType::NoExtern => Kind::Extern,
                HeapType::Any
                | HeapType::Eq
                | HeapType::I31
                | HeapType::Struct
                | HeapType::Array
                | HeapType::None => Kind::Any,
                HeapType::Exn | HeapType::NoExn => Kind::Exn,
                HeapType::Cont | HeapType::NoCont => Kind::Cont,
                HeapType::Concrete(handle) => match types.hierarchy(handle) {
                    Some(Hierarchy::Any) => Kind::Any,
                    Some(Hierarchy::Cont) => Kind::Cont,
                    Some(Hierarchy::Func) | None => Kind::Func,
                },
            },
        }
    }

    /// The kind of `value`.
    pub fn of_val(value: &Val) -> Kind {
        match value {
            Val::I32(_) => Kind::I32,
            Val::I64(_) => Kind::I64,
            Val::F32(_) => Kind::F32,
            Val::F64(_) => Kind::F64,
            Val::V128(_) => Kind::V128,
            Val::FuncRef(_) => Kind::Func,
            Val::ExternRef(_) => Kind::Extern,
            Val::AnyRef(_) => Kind::Any,
            Val::ExnRef(_) => Kind::Exn,
            Val::ContRef(_) => Kind::Cont,
        }
    }
}

/// [`Error::TypeMismatch`] where `value` is not a value of `ty`.
///
/// The check is by kind. A null fits every nullable reference type,
/// because the runtime layer makes the null of a concrete type a null
/// function reference whatever its hierarchy. The engine checks the rest,
/// such as the concrete type of a GC object.
pub fn check(value: &Val, ty: &ValType, types: &TypeRegistry) -> Result<()> {
    let null_fits = value.is_null() && ty.ref_type().is_some_and(|ty| ty.nullable);
    if null_fits || Kind::of_val(value) == Kind::of_type(ty, types) {
        Ok(())
    } else {
        Err(mismatch(format!("{value:?} is not a value of type {ty}")))
    }
}

/// `value` as the JavaScript API carries it.
pub fn to_js(objects: &Objects, value: &Val) -> Result<JsValue> {
    Ok(match value {
        Val::I32(value) => JsValue::from_f64(f64::from(*value)),
        Val::I64(value) => JsValue::from(*value),
        Val::F32(bits) => JsValue::from_f64(f64::from(f32::from_bits(*bits))),
        Val::F64(bits) => JsValue::from_f64(f64::from_bits(*bits)),
        Val::V128(_) => return Err(uncarried("v128")),
        Val::FuncRef(None)
        | Val::ExternRef(None)
        | Val::AnyRef(None)
        | Val::ExnRef(None)
        | Val::ContRef(None) => JsValue::NULL,
        Val::FuncRef(Some(func)) => objects.func(*func)?.function.clone().into(),
        Val::ExternRef(Some(extern_ref)) => objects.extern_ref(*extern_ref)?.value.clone(),
        Val::AnyRef(Some(any_ref)) => objects.any_ref(*any_ref)?.clone(),
        // The backend never hands out an `exnref` or a continuation
        // reference, so a handle to one names no object of this store.
        Val::ExnRef(Some(_)) | Val::ContRef(Some(_)) => return Err(Error::WrongStore),
    })
}

/// The value of kind `kind` that the JavaScript API gave as `value`.
///
/// A reference that crosses to the host takes a handle in `objects`, which
/// roots it for the life of the store.
pub fn from_js(objects: &mut Objects, value: JsValue, kind: Kind) -> Result<Val> {
    let wrong = |value: &JsValue| mismatch(format!("{value:?} is not a value of kind {kind:?}"));
    Ok(match kind {
        Kind::I32 => Val::I32(value.as_f64().ok_or_else(|| wrong(&value))? as i32),
        Kind::I64 => Val::I64(i64::try_from(value.clone()).map_err(|_| wrong(&value))?),
        Kind::F32 => Val::F32((value.as_f64().ok_or_else(|| wrong(&value))? as f32).to_bits()),
        Kind::F64 => Val::F64(value.as_f64().ok_or_else(|| wrong(&value))?.to_bits()),
        Kind::V128 => return Err(uncarried("v128")),
        Kind::Func if value.is_null() => Val::FuncRef(None),
        Kind::Func => match value.dyn_into::<Function>() {
            Ok(function) => Val::FuncRef(Some(objects.add_func(function, None))),
            Err(value) => return Err(wrong(&value)),
        },
        Kind::Extern if value.is_null() => Val::ExternRef(None),
        Kind::Extern => Val::ExternRef(Some(objects.add_extern_ref_value(value))),
        Kind::Any if value.is_null() => Val::AnyRef(None),
        Kind::Any => Val::AnyRef(Some(objects.add_any_ref(value))),
        Kind::Exn if value.is_null() => Val::ExnRef(None),
        Kind::Exn => return Err(uncarried("exnref")),
        Kind::Cont if value.is_null() => Val::ContRef(None),
        Kind::Cont => return Err(uncarried("continuation reference")),
    })
}

/// [`Error::TypeMismatch`] with `message`.
pub fn mismatch(message: String) -> Error {
    Error::TypeMismatch { message }
}

/// [`Error::TypeMismatch`] for a value of type `ty` that the JavaScript API
/// cannot carry between the host and a guest.
fn uncarried(ty: &str) -> Error {
    mismatch(format!(
        "the WebAssembly JavaScript API cannot carry a {ty} between the host and a guest"
    ))
}
