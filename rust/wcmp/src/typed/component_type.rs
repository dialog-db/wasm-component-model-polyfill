// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A Rust type as a component value type.

use wasmtime_environ::component::CanonicalAbiInfo;

use crate::types::ValueType;

use super::type_check_error::TypeCheckError;
use super::type_checker::TypeChecker;

/// A Rust type that stands for a component value type, as Wasmtime's
/// `ComponentType` does.
///
/// The trait carries the Canonical ABI layout of the type as
/// constants, and the type check that compares the Rust type with a
/// type a component declares. Its items are hidden: a host derives
/// the trait, or uses a type the polyfill implements it for, and does
/// not implement it by hand.
///
/// A typed value is data, so the trait requires `Send` and `Sync` on
/// every target. A JavaScript object in the browser is not data that
/// crosses the boundary: a host keeps it in a table and passes a
/// handle to it.
pub trait ComponentType: Send + Sync {
    /// The Canonical ABI layout of the type: its size and alignment
    /// in a 32-bit memory, and its count of flat values.
    #[doc(hidden)]
    const ABI: CanonicalAbiInfo;

    /// The size of the type in a 32-bit memory, in bytes.
    #[doc(hidden)]
    const SIZE32: usize = Self::ABI.size32 as usize;

    /// The alignment of the type in a 32-bit memory, in bytes.
    #[doc(hidden)]
    const ALIGN32: u32 = Self::ABI.align32;

    /// Whether the type is Rust's unit type, which a case of a
    /// `result` with no payload matches.
    #[doc(hidden)]
    const IS_RUST_UNIT_TYPE: bool = false;

    /// Compare the Rust type with `ty`, a type a component declares.
    /// The comparison has no subtyping: every count, name, and inner
    /// type has to match. A mismatch names the first part that does
    /// not match.
    #[doc(hidden)]
    fn typecheck(ty: &ValueType, types: &TypeChecker<'_>) -> Result<(), TypeCheckError>;
}
