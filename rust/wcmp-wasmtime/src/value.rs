//! Conversions between the scenario model's values and Wasmtime's.

use wasmtime::component::Val;
use wcmp_scenario::Value;

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
}
