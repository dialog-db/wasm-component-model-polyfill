// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The type of one argument or result of a call.

use core::fmt;

/// The type of a [`Value`](crate::Value): a scalar type or `string`.
///
/// A typed call picks its Rust types from these, because a runner
/// fixes the types of a typed function when it is compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// `bool`.
    Bool,
    /// `s8`.
    S8,
    /// `u8`.
    U8,
    /// `s16`.
    S16,
    /// `u16`.
    U16,
    /// `s32`.
    S32,
    /// `u32`.
    U32,
    /// `s64`.
    S64,
    /// `u64`.
    U64,
    /// `f32`.
    F32,
    /// `f64`.
    F64,
    /// `char`.
    Char,
    /// `string`.
    String,
}

impl ValueType {
    /// The type's name, as WIT spells it.
    pub fn name(self) -> &'static str {
        match self {
            ValueType::Bool => "bool",
            ValueType::S8 => "s8",
            ValueType::U8 => "u8",
            ValueType::S16 => "s16",
            ValueType::U16 => "u16",
            ValueType::S32 => "s32",
            ValueType::U32 => "u32",
            ValueType::S64 => "s64",
            ValueType::U64 => "u64",
            ValueType::F32 => "f32",
            ValueType::F64 => "f64",
            ValueType::Char => "char",
            ValueType::String => "string",
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
