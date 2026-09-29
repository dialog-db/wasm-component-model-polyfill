//! A value that crosses the boundary of a module.

use crate::externs::Func;
use crate::types::{HeapType, ValType};
use crate::values::{AnyRef, ContRef, ExnRef, ExternRef};

/// A value that crosses the boundary of a module, as Wasmtime's `Val` is.
///
/// A float holds its raw bits, so a NaN keeps its payload. A reference is
/// one case for each hierarchy of heap types, and `None` is its null.
#[derive(Clone, Copy, Debug)]
pub enum Val {
    /// A 32-bit integer.
    I32(i32),
    /// A 64-bit integer.
    I64(i64),
    /// The bits of a 32-bit float.
    F32(u32),
    /// The bits of a 64-bit float.
    F64(u64),
    /// A 128-bit vector.
    V128(u128),
    /// A function reference, or null.
    FuncRef(Option<Func>),
    /// An external reference, or null.
    ExternRef(Option<ExternRef>),
    /// An internal reference, or null.
    AnyRef(Option<AnyRef>),
    /// An exception reference, or null.
    ExnRef(Option<ExnRef>),
    /// A continuation reference, or null.
    ContRef(Option<ContRef>),
}

impl Val {
    /// The null of the hierarchy of `heap`.
    pub const fn null(heap: HeapType) -> Val {
        match heap {
            HeapType::Func | HeapType::NoFunc | HeapType::Concrete(_) => Val::FuncRef(None),
            HeapType::Extern | HeapType::NoExtern => Val::ExternRef(None),
            HeapType::Any
            | HeapType::Eq
            | HeapType::I31
            | HeapType::Struct
            | HeapType::Array
            | HeapType::None => Val::AnyRef(None),
            HeapType::Exn | HeapType::NoExn => Val::ExnRef(None),
            HeapType::Cont | HeapType::NoCont => Val::ContRef(None),
        }
    }

    /// The default value of `ty`: zero for a number or a vector, and null
    /// for a nullable reference. A reference that is not nullable has no
    /// default.
    ///
    /// The runtime layer cannot tell which hierarchy a concrete type belongs
    /// to, so the null of a nullable concrete reference is a null function
    /// reference. A backend that knows better makes its own null.
    pub const fn default_for_ty(ty: &ValType) -> Option<Val> {
        match ty {
            ValType::I32 => Some(Val::I32(0)),
            ValType::I64 => Some(Val::I64(0)),
            ValType::F32 => Some(Val::F32(0)),
            ValType::F64 => Some(Val::F64(0)),
            ValType::V128 => Some(Val::V128(0)),
            ValType::Ref(ref_type) if ref_type.nullable => Some(Val::null(ref_type.heap)),
            ValType::Ref(_) => None,
        }
    }

    /// The integer, where the value is an `i32`.
    pub const fn i32(&self) -> Option<i32> {
        match self {
            Val::I32(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer, where the value is an `i64`.
    pub const fn i64(&self) -> Option<i64> {
        match self {
            Val::I64(value) => Some(*value),
            _ => None,
        }
    }

    /// The float, where the value is an `f32`.
    pub const fn f32(&self) -> Option<f32> {
        match self {
            Val::F32(bits) => Some(f32::from_bits(*bits)),
            _ => None,
        }
    }

    /// The float, where the value is an `f64`.
    pub const fn f64(&self) -> Option<f64> {
        match self {
            Val::F64(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    /// The vector, where the value is a `v128`.
    pub const fn v128(&self) -> Option<u128> {
        match self {
            Val::V128(value) => Some(*value),
            _ => None,
        }
    }

    /// Whether the value is a null reference.
    pub const fn is_null(&self) -> bool {
        matches!(
            self,
            Val::FuncRef(None)
                | Val::ExternRef(None)
                | Val::AnyRef(None)
                | Val::ExnRef(None)
                | Val::ContRef(None)
        )
    }
}

impl From<i32> for Val {
    fn from(value: i32) -> Self {
        Val::I32(value)
    }
}

impl From<i64> for Val {
    fn from(value: i64) -> Self {
        Val::I64(value)
    }
}

impl From<f32> for Val {
    fn from(value: f32) -> Self {
        Val::F32(value.to_bits())
    }
}

impl From<f64> for Val {
    fn from(value: f64) -> Self {
        Val::F64(value.to_bits())
    }
}

impl From<Func> for Val {
    fn from(func: Func) -> Self {
        Val::FuncRef(Some(func))
    }
}

impl From<ExternRef> for Val {
    fn from(extern_ref: ExternRef) -> Self {
        Val::ExternRef(Some(extern_ref))
    }
}

impl From<AnyRef> for Val {
    fn from(any_ref: AnyRef) -> Self {
        Val::AnyRef(Some(any_ref))
    }
}
