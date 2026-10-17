// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How a type is printed.

/// Whether a type is printed as the value it is, or as the borrow an
/// export's call takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TypeMode {
    /// The value itself: `String`, `Vec<T>`, a named type.
    Owned,
    /// The shallow borrow an export takes, as Wasmtime's
    /// `AllBorrowed("'_")` with owning types: `&str`, `&[T]` of owned
    /// elements, and `&T` of a named type that holds a list.
    Borrowed,
}
