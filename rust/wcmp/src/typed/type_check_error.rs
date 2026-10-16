// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A failed type check, before it becomes an error of the polyfill.

use crate::error::{TypePathStep, TypeRendering};
use crate::internal::TypeCheckErrorInternal;
use crate::types::ValueType;

/// What a type check found where the Rust type and the declared type
/// differ: the path to the first part that differs, and what each side
/// has there.
///
/// The check builds this only when it fails. Each enclosing type adds
/// its own step as the failure leaves it, so the path grows from the
/// part that differs outward. The entry point that ran the check turns
/// it into [`Error::TypeMismatch`](crate::Error::TypeMismatch).
///
/// The parts are boxed, so that the `Result` of a check that passes,
/// which every check of a value that matches returns, stays small.
#[derive(Clone, Debug)]
pub struct TypeCheckError {
    parts: Box<Parts>,
}

/// The parts of a [`TypeCheckError`].
#[derive(Clone, Debug)]
struct Parts {
    /// The steps from the part that differs out to the value the check
    /// started at, innermost first.
    path: Vec<TypePathStep>,
    expected: TypeRendering,
    actual: TypeRendering,
}

impl TypeCheckError {
    /// A failure with an empty path.
    fn new(expected: TypeRendering, actual: TypeRendering) -> Self {
        Self {
            parts: Box::new(Parts {
                path: Vec::new(),
                expected,
                actual,
            }),
        }
    }

    /// The Rust type `R` does not stand for the declared type `ty`.
    pub fn mismatch<R: ?Sized>(ty: &ValueType) -> Self {
        Self::new(
            TypeRendering::Value(ty.clone()),
            TypeRendering::Rust(core::any::type_name::<R>().to_owned()),
        )
    }

    /// The declared type has `declared` members, cases, or names, and
    /// the Rust type has `rust`.
    pub fn count(declared: usize, rust: usize) -> Self {
        Self::new(TypeRendering::Arity(declared), TypeRendering::Arity(rust))
    }

    /// The declared type names a part `declared`, and the Rust type
    /// names it `rust`.
    pub fn name(declared: &str, rust: &str) -> Self {
        Self::new(
            TypeRendering::Name(declared.to_owned()),
            TypeRendering::Name(rust.to_owned()),
        )
    }

    /// The same failure, inside the part `step` names.
    pub fn at(mut self, step: TypePathStep) -> Self {
        self.parts.path.push(step);
        self
    }
}

impl TypeCheckErrorInternal for TypeCheckError {
    fn into_parts(self) -> (Vec<TypePathStep>, TypeRendering, TypeRendering) {
        let Parts {
            mut path,
            expected,
            actual,
        } = *self.parts;
        path.reverse();
        (path, expected, actual)
    }
}
