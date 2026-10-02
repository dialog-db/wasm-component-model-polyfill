// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
    /// Whether the type carries the `async` effect.
    ///
    /// A call of an `async` function is a task: the callee returns
    /// a status word rather than the lifted result, and produces
    /// the result through `task.return`. The polyfill reports the
    /// fact under the name Wasmtime gives it,
    /// `ComponentFunc::async_`. The typed conversion ignores the
    /// flag, because the parameters and the result of the two
    /// forms are the same.
    ///
    /// The flag is the callee's half of the call and says nothing
    /// about the caller's. How a caller reaches the function — a
    /// `canon lift` or a `canon lower`, each with or without its own
    /// `async` option — is the other axis, and the two move
    /// separately: an async-typed function may be lifted or lowered
    /// synchronously just as well. Only the reverse is constrained,
    /// because a canonical definition may declare the `async` option
    /// only for a function whose type carries the effect. A host
    /// reads this flag on an import exactly as it reads it on an
    /// export.
    pub async_: bool,
}

/// A single named parameter of a [`FunctionType`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionParameter {
    /// The parameter's name as declared by the function.
    pub name: String,
    /// The parameter's value type.
    pub ty: ValueType,
}
