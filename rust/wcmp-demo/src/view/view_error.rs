// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Why a list of nodes is not a view.

/// Why a list of nodes is not a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewError(pub String);

impl core::fmt::Display for ViewError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "the render returned no view: {}", self.0)
    }
}

impl std::error::Error for ViewError {}
