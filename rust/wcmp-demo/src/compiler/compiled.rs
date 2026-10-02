// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A compiled component.

/// The bytes of a compiled component, and how long the compile took.
pub struct Compiled {
    /// The component.
    pub bytes: Vec<u8>,
    /// The compile time, in milliseconds.
    pub millis: f64,
}
