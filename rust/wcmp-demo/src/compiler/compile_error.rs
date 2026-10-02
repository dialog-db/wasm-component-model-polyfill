// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Why a compile returned no component.

use wcmp::Error;

/// Why a compile returned no component.
#[derive(Debug)]
pub enum CompileError {
    /// The compiler refused the program. The text names the file, the
    /// line, and the column of each error.
    Diagnostics {
        /// The compiler's diagnostics.
        text: String,
        /// The compile time, in milliseconds.
        millis: f64,
    },
    /// The compiler could not run, or the polyfill failed under it.
    Failed(Error),
}

impl core::fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CompileError::Diagnostics { text, .. } => formatter.write_str(text.trim_end()),
            CompileError::Failed(error) => write!(formatter, "the compiler failed: {error}"),
        }
    }
}

impl From<Error> for CompileError {
    fn from(error: Error) -> Self {
        CompileError::Failed(error)
    }
}
