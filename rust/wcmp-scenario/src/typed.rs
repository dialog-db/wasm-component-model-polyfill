//! The Rust types of a typed call.

use crate::value::Value;
use crate::value_type::ValueType;

/// A Rust type that a typed call passes or returns: `bool`, one of the
/// integer and float types, `char`, or `String`.
///
/// A runner makes a typed call through a typed function whose Rust
/// types it fixes when it is compiled. It picks those types from the
/// [`ValueType`] of each argument and result, and moves each [`Value`]
/// in and out of them through this trait.
pub trait Typed: Sized {
    /// The type of the values this Rust type holds.
    const TYPE: ValueType;

    /// The Rust value of `value`, or `None` when `value` has another
    /// type.
    fn from_value(value: Value) -> Option<Self>;

    /// The value of `self`.
    fn into_value(self) -> Value;
}

macro_rules! typed {
    ($($rust:ty => $variant:ident),+ $(,)?) => {
        $(
            impl Typed for $rust {
                const TYPE: ValueType = ValueType::$variant;

                fn from_value(value: Value) -> Option<Self> {
                    match value {
                        Value::$variant(value) => Some(value),
                        _ => None,
                    }
                }

                fn into_value(self) -> Value {
                    Value::$variant(self)
                }
            }
        )+
    };
}

typed! {
    bool => Bool,
    i8 => S8,
    u8 => U8,
    i16 => S16,
    u16 => U16,
    i32 => S32,
    u32 => U32,
    i64 => S64,
    u64 => U64,
    f32 => F32,
    f64 => F64,
    char => Char,
    String => String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Typed>(value: Value) {
        assert_eq!(value.ty(), T::TYPE);
        let rust = T::from_value(value.clone()).unwrap();
        assert_eq!(rust.into_value(), value);
    }

    #[wcmp_macros::test]
    fn it_moves_every_value_into_its_rust_type_and_back() {
        round_trip::<bool>(Value::Bool(true));
        round_trip::<i8>(Value::S8(-8));
        round_trip::<u8>(Value::U8(8));
        round_trip::<i16>(Value::S16(-16));
        round_trip::<u16>(Value::U16(16));
        round_trip::<i32>(Value::S32(-32));
        round_trip::<u32>(Value::U32(32));
        round_trip::<i64>(Value::S64(-64));
        round_trip::<u64>(Value::U64(64));
        round_trip::<f32>(Value::F32(1.5));
        round_trip::<f64>(Value::F64(-0.0));
        round_trip::<char>(Value::Char('é'));
        round_trip::<String>(Value::String("hello".to_string()));
    }

    #[wcmp_macros::test]
    fn it_refuses_a_value_of_another_type() {
        assert_eq!(i32::from_value(Value::U32(3)), None);
        assert_eq!(String::from_value(Value::Char('a')), None);
    }
}
