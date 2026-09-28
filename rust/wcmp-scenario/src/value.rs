//! One argument or result of a call.

use core::fmt;

use crate::syntax::quote;
use crate::value_type::ValueType;

/// One argument or result of a call: a scalar or a string.
///
/// A number carries its type, because the file that holds it is written
/// by hand without the component's signature at hand, and a typed call
/// needs the exact type.
///
/// Two floats are equal when their bits are, except that every NaN
/// equals every other NaN: a NaN's payload is not something a scenario
/// can promise, while the sign of a zero is.
#[derive(Debug, Clone)]
pub enum Value {
    /// A `bool`.
    Bool(bool),
    /// An `s8`.
    S8(i8),
    /// A `u8`.
    U8(u8),
    /// An `s16`.
    S16(i16),
    /// A `u16`.
    U16(u16),
    /// An `s32`.
    S32(i32),
    /// A `u32`.
    U32(u32),
    /// An `s64`.
    S64(i64),
    /// A `u64`.
    U64(u64),
    /// An `f32`.
    F32(f32),
    /// An `f64`.
    F64(f64),
    /// A `char`.
    Char(char),
    /// A `string`.
    String(String),
}

impl Value {
    /// The value's type.
    pub fn ty(&self) -> ValueType {
        match self {
            Value::Bool(_) => ValueType::Bool,
            Value::S8(_) => ValueType::S8,
            Value::U8(_) => ValueType::U8,
            Value::S16(_) => ValueType::S16,
            Value::U16(_) => ValueType::U16,
            Value::S32(_) => ValueType::S32,
            Value::U32(_) => ValueType::U32,
            Value::S64(_) => ValueType::S64,
            Value::U64(_) => ValueType::U64,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
            Value::Char(_) => ValueType::Char,
            Value::String(_) => ValueType::String,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::S8(a), Value::S8(b)) => a == b,
            (Value::U8(a), Value::U8(b)) => a == b,
            (Value::S16(a), Value::S16(b)) => a == b,
            (Value::U16(a), Value::U16(b)) => a == b,
            (Value::S32(a), Value::S32(b)) => a == b,
            (Value::U32(a), Value::U32(b)) => a == b,
            (Value::S64(a), Value::S64(b)) => a == b,
            (Value::U64(a), Value::U64(b)) => a == b,
            (Value::F32(a), Value::F32(b)) => {
                (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
            }
            (Value::F64(a), Value::F64(b)) => {
                (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
            }
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(value) => write!(formatter, "{value}"),
            Value::S8(value) => write!(formatter, "{value}s8"),
            Value::U8(value) => write!(formatter, "{value}u8"),
            Value::S16(value) => write!(formatter, "{value}s16"),
            Value::U16(value) => write!(formatter, "{value}u16"),
            Value::S32(value) => write!(formatter, "{value}s32"),
            Value::U32(value) => write!(formatter, "{value}u32"),
            Value::S64(value) => write!(formatter, "{value}s64"),
            Value::U64(value) => write!(formatter, "{value}u64"),
            // `Display` for a float is the shortest text that reads back
            // as the same value, and `NaN`, `inf`, and `-inf` read back.
            Value::F32(value) => write!(formatter, "{value}f32"),
            Value::F64(value) => write!(formatter, "{value}f64"),
            Value::Char(value) => formatter.write_str(&quote(&value.to_string(), '\'')),
            Value::String(value) => formatter.write_str(&quote(value, '"')),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_equates_any_two_nans_but_not_two_signed_zeros() {
        assert_eq!(Value::F32(f32::NAN), Value::F32(-f32::NAN));
        assert_eq!(Value::F64(f64::NAN), Value::F64(-f64::NAN));
        assert_ne!(Value::F64(0.0), Value::F64(-0.0));
        assert_ne!(Value::F32(1.0), Value::F64(1.0));
    }

    #[wcmp_macros::test]
    fn it_tells_apart_two_numbers_of_different_types() {
        assert_ne!(Value::U32(3), Value::S32(3));
    }
}
