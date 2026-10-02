// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A continuation reference.

handle! {
    /// A `contref`: a reference to a continuation of stack switching.
    ///
    /// The reference is opaque. The host can hold it, test it for null (a
    /// null is `None` in a [`Val`](crate::Val)), and give it back to a guest
    /// of the same store.
    ContRef
}
