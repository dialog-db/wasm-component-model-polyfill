//! Conversions between the scenario model's values and Wasmtime's.

use wasmtime::component::{Type, Val};
use wcmp_scenario::{Value, ValueType};

/// The Wasmtime value of an argument.
pub fn to_val(value: &Value) -> Val {
    match value {
        Value::Bool(value) => Val::Bool(*value),
        Value::S8(value) => Val::S8(*value),
        Value::U8(value) => Val::U8(*value),
        Value::S16(value) => Val::S16(*value),
        Value::U16(value) => Val::U16(*value),
        Value::S32(value) => Val::S32(*value),
        Value::U32(value) => Val::U32(*value),
        Value::S64(value) => Val::S64(*value),
        Value::U64(value) => Val::U64(*value),
        Value::F32(value) => Val::Float32(*value),
        Value::F64(value) => Val::Float64(*value),
        Value::Char(value) => Val::Char(*value),
        Value::String(value) => Val::String(value.clone()),
    }
}

/// The scenario model's value of a result, or `None` when the model has
/// no value of its type: it holds scalars and strings only.
pub fn from_val(val: &Val) -> Option<Value> {
    Some(match val {
        Val::Bool(value) => Value::Bool(*value),
        Val::S8(value) => Value::S8(*value),
        Val::U8(value) => Value::U8(*value),
        Val::S16(value) => Value::S16(*value),
        Val::U16(value) => Value::U16(*value),
        Val::S32(value) => Value::S32(*value),
        Val::U32(value) => Value::U32(*value),
        Val::S64(value) => Value::S64(*value),
        Val::U64(value) => Value::U64(*value),
        Val::Float32(value) => Value::F32(*value),
        Val::Float64(value) => Value::F64(*value),
        Val::Char(value) => Value::Char(*value),
        Val::String(value) => Value::String(value.clone()),
        _ => return None,
    })
}

/// The scenario model's type of a Wasmtime type, or `None` when the
/// model has no values of it: it holds scalars and strings only.
pub fn value_type(ty: &Type) -> Option<ValueType> {
    Some(match ty {
        Type::Bool => ValueType::Bool,
        Type::S8 => ValueType::S8,
        Type::U8 => ValueType::U8,
        Type::S16 => ValueType::S16,
        Type::U16 => ValueType::U16,
        Type::S32 => ValueType::S32,
        Type::U32 => ValueType::U32,
        Type::S64 => ValueType::S64,
        Type::U64 => ValueType::U64,
        Type::Float32 => ValueType::F32,
        Type::Float64 => ValueType::F64,
        Type::Char => ValueType::Char,
        Type::String => ValueType::String,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_converts_every_value_there_and_back() {
        let values = [
            Value::Bool(true),
            Value::S8(-8),
            Value::U8(8),
            Value::S16(-16),
            Value::U16(16),
            Value::S32(-32),
            Value::U32(32),
            Value::S64(-64),
            Value::U64(64),
            Value::F32(1.5),
            Value::F64(-0.0),
            Value::Char('é'),
            Value::String("hello".to_string()),
        ];
        for value in values {
            assert_eq!(from_val(&to_val(&value)), Some(value));
        }
    }

    #[wcmp_macros::test]
    fn it_has_no_value_for_a_compound_result() {
        assert_eq!(from_val(&Val::List(vec![Val::U8(1)])), None);
    }

    #[wcmp_macros::test]
    fn it_names_the_type_of_a_scalar_or_a_string_and_of_nothing_else() {
        assert_eq!(value_type(&Type::S32), Some(ValueType::S32));
        assert_eq!(value_type(&Type::Float64), Some(ValueType::F64));
        assert_eq!(value_type(&Type::String), Some(ValueType::String));
        assert_eq!(value_type(&Type::ErrorContext), None);
    }
}
