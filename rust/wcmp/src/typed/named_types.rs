// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The declared types of a function's parameter list or result list.

use crate::component::FunctionType;
use crate::error::{AbiPosition, TypePathStep};
use crate::types::ValueType;

/// The declared types of one list of a function: its parameters, or
/// its result, which is a list of zero or one types.
///
/// A tuple that is the list crosses member by member against these
/// types, and each member's failure names the parameter or the result
/// it is. The same tuple inside another value is a WIT `tuple`, whose
/// members a [`ValueType::Tuple`] declares instead.
#[derive(Clone, Copy, Debug)]
pub struct NamedTypes<'a> {
    ty: &'a FunctionType,
    results: bool,
}

impl<'a> NamedTypes<'a> {
    /// The parameter list of `ty`.
    pub fn parameters(ty: &'a FunctionType) -> Self {
        Self { ty, results: false }
    }

    /// The result list of `ty`.
    pub fn results(ty: &'a FunctionType) -> Self {
        Self { ty, results: true }
    }

    /// How many types the list declares.
    pub fn count(&self) -> usize {
        if self.results {
            usize::from(self.ty.result.is_some())
        } else {
            self.ty.parameters.len()
        }
    }

    /// The declared type of member `index`.
    pub fn get(&self, index: usize) -> Option<&'a ValueType> {
        if self.results {
            self.ty.result.as_ref().filter(|_| index == 0)
        } else {
            self.ty.parameters.get(index).map(|parameter| &parameter.ty)
        }
    }

    /// The slot a crossing of member `index` reports a failure at.
    pub fn position(&self, index: usize) -> AbiPosition {
        if self.results {
            AbiPosition::Result
        } else {
            AbiPosition::Argument(index)
        }
    }

    /// The step a type mismatch of member `index` names.
    pub fn step(&self, index: usize) -> TypePathStep {
        if self.results {
            TypePathStep::Result(index)
        } else {
            TypePathStep::Parameter(index)
        }
    }
}
