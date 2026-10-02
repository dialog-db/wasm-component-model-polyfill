// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A concrete heap type.

use core::fmt;

use crate::contract::RawTypeHandle;

/// A concrete heap type: a function, struct, array, or continuation type
/// that a module defines.
///
/// The handle is opaque. The host can print it, and can compare two handles
/// from one engine: they are equal exactly when the engine takes them for
/// the same type. A comparison of handles from two engines means nothing.
/// The runtime layer checks no subtypes. The engine does that when it links
/// a module.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeHandle {
    raw: u64,
}

impl RawTypeHandle for TypeHandle {
    fn from_raw(raw: u64) -> Self {
        Self { raw }
    }

    fn raw(&self) -> u64 {
        self.raw
    }
}

impl fmt::Display for TypeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "type#{}", self.raw)
    }
}

impl fmt::Debug for TypeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeHandle({self})")
    }
}
