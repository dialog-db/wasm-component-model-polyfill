// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Component-level runtime values.
//!
//! Where [`crate::types`] describes the *shape* a value occupies in
//! the component type system, this module describes the values
//! themselves — the data that travels through a function export call
//! across the polyfill's public API.
//!
//! Every variant in [`Val`] mirrors a shape in
//! [`crate::ValueType`]. Compound variants carry owned, polyfill-
//! typed payloads — a `Val::List` is a `Box<[Val]>`, a `Val::Record`
//! is a `Box<[ValField]>`, and so on — so a `Val` can be passed
//! across an export call without borrowing into the runtime
//! substrate's memory.

mod val;

pub use val::{Val, ValField};
