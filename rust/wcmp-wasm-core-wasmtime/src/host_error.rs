// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The error of a host function, on its way through Wasmtime.

use core::fmt;

/// The error a host function returned, wrapped so that it crosses Wasmtime
/// and comes back out unchanged.
///
/// Wasmtime returns a host function's error from the call that ran the
/// guest, with context of its own around it. The backend wraps the error
/// in this type on the way in, and finds the wrapper again on the way out,
/// so the host receives exactly the error its function returned, and no
/// other error of Wasmtime's is taken for one.
pub struct HostError(pub anyhow::Error);

impl fmt::Debug for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl core::error::Error for HostError {}
