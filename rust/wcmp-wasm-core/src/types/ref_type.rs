// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A reference type.

use core::fmt;

use crate::types::HeapType;

/// A reference type: a heap type, and whether null is a value of the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RefType {
    /// Whether null is a value of the type.
    pub nullable: bool,
    /// The heap type the reference points into.
    pub heap: HeapType,
}

impl RefType {
    /// The reference type to `heap`, nullable or not.
    pub const fn new(nullable: bool, heap: HeapType) -> Self {
        Self { nullable, heap }
    }

    /// `funcref`: `(ref null func)`.
    pub const FUNCREF: RefType = RefType::new(true, HeapType::Func);
    /// `externref`: `(ref null extern)`.
    pub const EXTERNREF: RefType = RefType::new(true, HeapType::Extern);
    /// `anyref`: `(ref null any)`.
    pub const ANYREF: RefType = RefType::new(true, HeapType::Any);
    /// `eqref`: `(ref null eq)`.
    pub const EQREF: RefType = RefType::new(true, HeapType::Eq);
    /// `i31ref`: `(ref null i31)`.
    pub const I31REF: RefType = RefType::new(true, HeapType::I31);
    /// `structref`: `(ref null struct)`.
    pub const STRUCTREF: RefType = RefType::new(true, HeapType::Struct);
    /// `arrayref`: `(ref null array)`.
    pub const ARRAYREF: RefType = RefType::new(true, HeapType::Array);
    /// `exnref`: `(ref null exn)`.
    pub const EXNREF: RefType = RefType::new(true, HeapType::Exn);
    /// `contref`: `(ref null cont)`.
    pub const CONTREF: RefType = RefType::new(true, HeapType::Cont);
    /// `nullfuncref`: `(ref null nofunc)`.
    pub const NULLFUNCREF: RefType = RefType::new(true, HeapType::NoFunc);
    /// `nullexternref`: `(ref null noextern)`.
    pub const NULLEXTERNREF: RefType = RefType::new(true, HeapType::NoExtern);
    /// `nullref`: `(ref null none)`.
    pub const NULLREF: RefType = RefType::new(true, HeapType::None);
    /// `nullexnref`: `(ref null noexn)`.
    pub const NULLEXNREF: RefType = RefType::new(true, HeapType::NoExn);
    /// `nullcontref`: `(ref null nocont)`.
    pub const NULLCONTREF: RefType = RefType::new(true, HeapType::NoCont);
}

impl fmt::Display for RefType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.nullable {
            write!(f, "(ref null {})", self.heap)
        } else {
            write!(f, "(ref {})", self.heap)
        }
    }
}
