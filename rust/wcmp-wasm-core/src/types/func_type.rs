// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of a function.

use crate::types::ValType;

/// The type of a function: its parameters and its results.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FuncType {
    params: Box<[ValType]>,
    results: Box<[ValType]>,
}

impl FuncType {
    /// The function type from `params` to `results`.
    pub fn new(
        params: impl IntoIterator<Item = ValType>,
        results: impl IntoIterator<Item = ValType>,
    ) -> Self {
        Self {
            params: params.into_iter().collect(),
            results: results.into_iter().collect(),
        }
    }

    /// The types of the parameters, in order.
    pub fn params(&self) -> &[ValType] {
        &self.params
    }

    /// The types of the results, in order.
    pub fn results(&self) -> &[ValType] {
        &self.results
    }
}
