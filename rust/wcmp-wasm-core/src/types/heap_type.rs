// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The heap type of a reference.

use core::fmt;

use crate::types::TypeHandle;

/// The heap type of a reference type: every abstract heap type of Wasm 3.0,
/// or a concrete type.
///
/// The abstract heap types form five hierarchies, each with its bottom
/// type: `func` over `nofunc`, `extern` over `noextern`, `any` (with `eq`,
/// `i31`, `struct`, and `array` below it) over `none`, `exn` over `noexn`,
/// and `cont` over `nocont`. A concrete type is a [`TypeHandle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeapType {
    /// `func`: every function.
    Func,
    /// `extern`: every external reference.
    Extern,
    /// `any`: every internal reference.
    Any,
    /// `eq`: every internal reference that can be compared.
    Eq,
    /// `i31`: every unboxed 31-bit integer.
    I31,
    /// `struct`: every struct.
    Struct,
    /// `array`: every array.
    Array,
    /// `exn`: every exception.
    Exn,
    /// `cont`: every continuation.
    Cont,
    /// `nofunc`: the bottom of the function hierarchy.
    NoFunc,
    /// `noextern`: the bottom of the external hierarchy.
    NoExtern,
    /// `none`: the bottom of the internal hierarchy.
    None,
    /// `noexn`: the bottom of the exception hierarchy.
    NoExn,
    /// `nocont`: the bottom of the continuation hierarchy.
    NoCont,
    /// A concrete type that a module defines.
    Concrete(TypeHandle),
}

impl fmt::Display for HeapType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            HeapType::Func => "func",
            HeapType::Extern => "extern",
            HeapType::Any => "any",
            HeapType::Eq => "eq",
            HeapType::I31 => "i31",
            HeapType::Struct => "struct",
            HeapType::Array => "array",
            HeapType::Exn => "exn",
            HeapType::Cont => "cont",
            HeapType::NoFunc => "nofunc",
            HeapType::NoExtern => "noextern",
            HeapType::None => "none",
            HeapType::NoExn => "noexn",
            HeapType::NoCont => "nocont",
            HeapType::Concrete(handle) => return fmt::Display::fmt(handle, f),
        };
        f.write_str(name)
    }
}
