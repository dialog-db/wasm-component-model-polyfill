// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The projection from a store's host data to the part of it that one
//! host crate serves.
//!
//! A crate that serves an interface does not know the `T` of the
//! embedder's `Store<T>`. It names the part of the host data it works
//! on through a [`HasData`] type instead, and the embedder hands it a
//! function that reaches that part from the whole host data. These are
//! Wasmtime's types, with Wasmtime's names.

mod access;
mod has_data;
mod has_self;

pub use access::Access;
pub use has_data::HasData;
pub use has_self::HasSelf;
