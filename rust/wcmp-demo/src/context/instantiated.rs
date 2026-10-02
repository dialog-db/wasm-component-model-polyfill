// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A component instantiated from a compile.

use wcmp::{Instance, Store};

/// A component instantiated from a compile, and how long that took.
pub struct Instantiated<T: 'static> {
    /// The store the instance lives in.
    pub store: Store<T>,
    /// The instance.
    pub instance: Instance,
    /// The instantiate time, in milliseconds: the parse of the bytes,
    /// the link, and the instantiation.
    pub millis: f64,
}
