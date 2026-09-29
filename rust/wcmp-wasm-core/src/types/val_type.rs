//! The type of a value.

use core::fmt;

use crate::types::RefType;

/// The type of a value that crosses the boundary of a module: every value
/// type of Wasm 3.0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValType {
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
    /// A reference.
    Ref(RefType),
}

impl ValType {
    /// `funcref`: `(ref null func)`.
    pub const FUNCREF: ValType = ValType::Ref(RefType::FUNCREF);
    /// `externref`: `(ref null extern)`.
    pub const EXTERNREF: ValType = ValType::Ref(RefType::EXTERNREF);
    /// `anyref`: `(ref null any)`.
    pub const ANYREF: ValType = ValType::Ref(RefType::ANYREF);
    /// `eqref`: `(ref null eq)`.
    pub const EQREF: ValType = ValType::Ref(RefType::EQREF);
    /// `i31ref`: `(ref null i31)`.
    pub const I31REF: ValType = ValType::Ref(RefType::I31REF);
    /// `structref`: `(ref null struct)`.
    pub const STRUCTREF: ValType = ValType::Ref(RefType::STRUCTREF);
    /// `arrayref`: `(ref null array)`.
    pub const ARRAYREF: ValType = ValType::Ref(RefType::ARRAYREF);
    /// `exnref`: `(ref null exn)`.
    pub const EXNREF: ValType = ValType::Ref(RefType::EXNREF);
    /// `contref`: `(ref null cont)`.
    pub const CONTREF: ValType = ValType::Ref(RefType::CONTREF);
    /// `nullfuncref`: `(ref null nofunc)`.
    pub const NULLFUNCREF: ValType = ValType::Ref(RefType::NULLFUNCREF);
    /// `nullexternref`: `(ref null noextern)`.
    pub const NULLEXTERNREF: ValType = ValType::Ref(RefType::NULLEXTERNREF);
    /// `nullref`: `(ref null none)`.
    pub const NULLREF: ValType = ValType::Ref(RefType::NULLREF);
    /// `nullexnref`: `(ref null noexn)`.
    pub const NULLEXNREF: ValType = ValType::Ref(RefType::NULLEXNREF);
    /// `nullcontref`: `(ref null nocont)`.
    pub const NULLCONTREF: ValType = ValType::Ref(RefType::NULLCONTREF);

    /// Whether the type is a number type: `i32`, `i64`, `f32`, or `f64`.
    pub const fn is_num(&self) -> bool {
        matches!(
            self,
            ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64
        )
    }

    /// The reference type, where the type is one.
    pub const fn ref_type(&self) -> Option<&RefType> {
        match self {
            ValType::Ref(ref_type) => Some(ref_type),
            _ => None,
        }
    }
}

impl From<RefType> for ValType {
    fn from(ref_type: RefType) -> Self {
        ValType::Ref(ref_type)
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValType::I32 => f.write_str("i32"),
            ValType::I64 => f.write_str("i64"),
            ValType::F32 => f.write_str("f32"),
            ValType::F64 => f.write_str("f64"),
            ValType::V128 => f.write_str("v128"),
            ValType::Ref(ref_type) => fmt::Display::fmt(ref_type, f),
        }
    }
}
