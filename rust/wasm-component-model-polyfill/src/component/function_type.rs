//! The structural shape of a component-level function type.

use crate::types::ValueType;

/// A component-level function signature: an ordered list of named
/// parameters and an optional result type.
///
/// Component functions are not [`ValueType`]s — they cannot appear
/// inside a record or list — but they are first-class extern types
/// that imports and exports can carry. Their parameter and result
/// shapes themselves are [`ValueType`]s, so structural equality on
/// function types reduces to structural equality on each component.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionType {
    /// The function's parameters, in declaration order.
    pub parameters: Vec<FunctionParameter>,
    /// The function's optional result type.
    pub result: Option<ValueType>,
}

/// A single named parameter of a [`FunctionType`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionParameter {
    /// The parameter's name as declared by the function.
    pub name: String,
    /// The parameter's value type.
    pub ty: ValueType,
}
