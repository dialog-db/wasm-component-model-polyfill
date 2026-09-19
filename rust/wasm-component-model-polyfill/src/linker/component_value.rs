//! Conversion between statically-typed Rust values and the polyfill's
//! [`Val`] enum.
//!
//! The [`ComponentValue`] trait names a Rust type that participates
//! in the polyfill's typed host-function and typed export-call
//! surfaces. Each implementation supplies (a) the static
//! [`ValueType`] the corresponding component-level type carries,
//! (b) `from_val` for receiving values across the boundary, and
//! (c) `to_val` for sending them.
//!
//! [`ComponentParameters`] and [`ComponentResult`] erase the
//! statically-typed argument tuple and return into the same `Val`
//! shape the canonical-ABI machinery operates on. Both traits are
//! implemented for the small set of arities the synchronous
//! baseline tests exercise; extending to higher arities is purely
//! mechanical.

use std::collections::HashMap;
use std::hash::Hash;

use crate::component::{FunctionParameter, FunctionType};
use crate::error::{
    AbiCause, AbiError, AbiPosition, Error, Result, TypeMismatch, TypeMismatchPosition,
    TypeRendering,
};
use crate::types::{FixedLengthListType, ListType, MapType, OptionType, PrimitiveType, ValueType};
use crate::value::Val;

/// A Rust type that maps to a single component-level value type.
pub trait ComponentValue: Sized + Send + Sync + 'static {
    /// The component-level [`ValueType`] this Rust type maps to.
    fn value_type() -> ValueType;
    /// Decode a [`Val`] into this Rust type.
    fn from_val(val: &Val) -> Result<Self>;
    /// Encode this Rust value as a [`Val`].
    fn to_val(self) -> Val;
}

/// A statically-typed argument tuple.
///
/// Implemented for the empty tuple and for a small set of arities
/// covering the synchronous baseline tests. Each implementation
/// derives the function's parameter list from the constituent
/// [`ComponentValue`] implementations.
pub trait ComponentParameters: Sized + Send + Sync + 'static {
    /// The parameter list this tuple corresponds to. Names are
    /// synthesised as `arg0`, `arg1`, … because the host's binding
    /// does not preserve them.
    fn parameter_types() -> Vec<FunctionParameter>;
    /// Decode a slice of [`Val`]s into this tuple.
    fn from_vals(vals: &[Val]) -> Result<Self>;
    /// Encode this tuple into the slice of [`Val`]s the canonical
    /// ABI lower path consumes. The order matches
    /// [`Self::parameter_types`].
    fn into_vals(self) -> Vec<Val>;
}

/// A statically-typed return value.
pub trait ComponentResult: Sized + Send + Sync + 'static {
    /// The component-level result type this Rust type maps to.
    /// `None` denotes "no result"; the synchronous baseline allows
    /// at most one result.
    fn result_type() -> Option<ValueType>;
    /// Encode this Rust value as the optional result [`Val`].
    fn into_val(self) -> Option<Val>;
    /// Decode the optional result [`Val`] the canonical-ABI lift
    /// produced into this Rust value. `None` means the export
    /// declared no result; `Some(val)` carries the lifted value.
    fn from_val(val: Option<&Val>) -> Result<Self>;
}

// === ComponentValue impls for primitives ===

macro_rules! impl_primitive_value {
    ($rust:ty, $variant:ident, $primitive:ident) => {
        impl ComponentValue for $rust {
            fn value_type() -> ValueType {
                ValueType::Primitive(PrimitiveType::$primitive)
            }
            fn from_val(val: &Val) -> Result<Self> {
                match val {
                    Val::$variant(v) => Ok(v.clone()),
                    _ => Err(value_mismatch(val)),
                }
            }
            fn to_val(self) -> Val {
                Val::$variant(self)
            }
        }
    };
}

impl_primitive_value!(bool, Bool, Bool);
impl_primitive_value!(i8, S8, S8);
impl_primitive_value!(u8, U8, U8);
impl_primitive_value!(i16, S16, S16);
impl_primitive_value!(u16, U16, U16);
impl_primitive_value!(i32, S32, S32);
impl_primitive_value!(u32, U32, U32);
impl_primitive_value!(i64, S64, S64);
impl_primitive_value!(u64, U64, U64);
impl_primitive_value!(f32, F32, F32);
impl_primitive_value!(f64, F64, F64);
impl_primitive_value!(char, Char, Char);
impl_primitive_value!(String, String, String);

impl<T: ComponentValue> ComponentValue for Vec<T> {
    fn value_type() -> ValueType {
        ValueType::List(ListType::new(T::value_type()))
    }
    fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::List(items) => items.iter().map(T::from_val).collect(),
            _ => Err(value_mismatch(val)),
        }
    }
    fn to_val(self) -> Val {
        Val::List(self.into_iter().map(T::to_val).collect())
    }
}

/// A `list<T, N>` as a Rust array of `N` elements.
impl<T: ComponentValue, const N: usize> ComponentValue for [T; N] {
    fn value_type() -> ValueType {
        ValueType::FixedLengthList(FixedLengthListType::new(T::value_type(), N as u32))
    }
    fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::FixedLengthList(items) if items.len() == N => {
                let items: Vec<T> = items.iter().map(T::from_val).collect::<Result<_>>()?;
                items.try_into().map_err(|_| value_mismatch(val))
            }
            _ => Err(value_mismatch(val)),
        }
    }
    fn to_val(self) -> Val {
        Val::FixedLengthList(self.into_iter().map(T::to_val).collect())
    }
}

/// A `map<K, V>` as a Rust hash map, as Wasmtime maps it. Entries
/// cross the boundary in the map's iteration order; a value lifted
/// with a duplicate key keeps the last entry.
impl<K: ComponentValue + Eq + Hash, V: ComponentValue> ComponentValue for HashMap<K, V> {
    fn value_type() -> ValueType {
        ValueType::Map(MapType::new(K::value_type(), V::value_type()))
    }
    fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::Map(entries) => entries
                .iter()
                .map(|(key, value)| Ok((K::from_val(key)?, V::from_val(value)?)))
                .collect(),
            _ => Err(value_mismatch(val)),
        }
    }
    fn to_val(self) -> Val {
        Val::Map(
            self.into_iter()
                .map(|(key, value)| (key.to_val(), value.to_val()))
                .collect(),
        )
    }
}

impl<T: ComponentValue> ComponentValue for Option<T> {
    fn value_type() -> ValueType {
        ValueType::Option(OptionType::new(T::value_type()))
    }
    fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::Option(None) => Ok(None),
            Val::Option(Some(inner)) => T::from_val(inner).map(Some),
            _ => Err(value_mismatch(val)),
        }
    }
    fn to_val(self) -> Val {
        Val::Option(self.map(|inner| Box::new(inner.to_val())))
    }
}

// === ComponentParameters impls ===

impl ComponentParameters for () {
    fn parameter_types() -> Vec<FunctionParameter> {
        Vec::new()
    }
    fn from_vals(vals: &[Val]) -> Result<Self> {
        if vals.is_empty() {
            Ok(())
        } else {
            Err(arity_mismatch(0, vals.len()))
        }
    }
    fn into_vals(self) -> Vec<Val> {
        Vec::new()
    }
}

macro_rules! impl_component_parameters {
    ($count:literal, $( ($index:tt, $name:ident) ),+ ) => {
        impl<$( $name: ComponentValue ),+> ComponentParameters for ($( $name, )+) {
            fn parameter_types() -> Vec<FunctionParameter> {
                vec![
                    $( FunctionParameter {
                        name: format!("arg{}", $index),
                        ty: $name::value_type(),
                    }, )+
                ]
            }
            fn from_vals(vals: &[Val]) -> Result<Self> {
                if vals.len() != $count {
                    return Err(arity_mismatch($count, vals.len()));
                }
                Ok(( $( $name::from_val(&vals[$index])?, )+ ))
            }
            fn into_vals(self) -> Vec<Val> {
                vec![ $( self.$index.to_val(), )+ ]
            }
        }
    };
}

impl_component_parameters!(1, (0, A));
impl_component_parameters!(2, (0, A), (1, B));
impl_component_parameters!(3, (0, A), (1, B), (2, C));
impl_component_parameters!(4, (0, A), (1, B), (2, C), (3, D));

// === ComponentResult impls ===

impl ComponentResult for () {
    fn result_type() -> Option<ValueType> {
        None
    }
    fn into_val(self) -> Option<Val> {
        None
    }
    fn from_val(val: Option<&Val>) -> Result<Self> {
        match val {
            None => Ok(()),
            Some(_) => Err(unexpected_result_present()),
        }
    }
}

impl<T: ComponentValue> ComponentResult for T {
    fn result_type() -> Option<ValueType> {
        Some(<T as ComponentValue>::value_type())
    }
    fn into_val(self) -> Option<Val> {
        Some(<T as ComponentValue>::to_val(self))
    }
    fn from_val(val: Option<&Val>) -> Result<Self> {
        match val {
            Some(v) => <T as ComponentValue>::from_val(v),
            None => Err(missing_result()),
        }
    }
}

// === Helpers ===

/// Build a [`FunctionType`] from a [`ComponentParameters`] +
/// [`ComponentResult`] pair. A host function is synchronous, so the
/// type it derives never carries the `async` effect.
pub fn function_type_for<P: ComponentParameters, R: ComponentResult>() -> FunctionType {
    FunctionType {
        parameters: P::parameter_types(),
        result: R::result_type(),
        async_: false,
    }
}

fn value_mismatch(_val: &Val) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(0),
        valtype: Some(ValueType::Primitive(PrimitiveType::Bool)),
        cause: AbiCause::HostValueMismatch,
    })
}

fn arity_mismatch(expected: usize, found: usize) -> Error {
    Error::from(TypeMismatch {
        position: TypeMismatchPosition::TypedExportCall {
            export: "<typed-call>".to_owned(),
        },
        expected: TypeRendering::Function(FunctionType {
            parameters: vec![FunctionParameter {
                name: format!("expected={expected}"),
                ty: ValueType::Primitive(PrimitiveType::Bool),
            }],
            result: None,
            async_: false,
        }),
        actual: TypeRendering::Function(FunctionType {
            parameters: vec![FunctionParameter {
                name: format!("found={found}"),
                ty: ValueType::Primitive(PrimitiveType::Bool),
            }],
            result: None,
            async_: false,
        }),
    })
}

fn missing_result() -> Error {
    Error::from(AbiError {
        position: AbiPosition::Result,
        valtype: Some(ValueType::Primitive(PrimitiveType::Bool)),
        cause: AbiCause::HostValueMismatch,
    })
}

fn unexpected_result_present() -> Error {
    Error::from(AbiError {
        position: AbiPosition::Result,
        valtype: Some(ValueType::Primitive(PrimitiveType::Bool)),
        cause: AbiCause::HostValueMismatch,
    })
}
