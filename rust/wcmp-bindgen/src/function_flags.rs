// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The flags the `imports` and `exports` options give a function.

use core::ops::{BitOr, BitOrAssign};

/// The set of flags the `imports` and `exports` options give one
/// function: Wasmtime's six (`crates/wit-bindgen/src/config.rs` at
/// `v49.0.0-rc.1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FunctionFlags(u8);

impl FunctionFlags {
    /// `async`.
    pub const ASYNC: Self = Self(1 << 0);
    /// `trappable`: the method returns a `Result`, and an `Err` traps.
    pub const TRAPPABLE: Self = Self(1 << 1);
    /// `store`: the method reaches the store.
    pub const STORE: Self = Self(1 << 2);
    /// `tracing`: the call opens a span and records its arguments.
    pub const TRACING: Self = Self(1 << 3);
    /// `verbose_tracing`: the span records lists in full.
    pub const VERBOSE_TRACING: Self = Self(1 << 4);
    /// `exact`: the rule matches only the full name it gives.
    pub const EXACT: Self = Self(1 << 5);

    /// No flags.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Whether every flag of `other` is set here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags of this set, by their names in an invocation, in
    /// Wasmtime's order.
    pub fn names(self) -> Vec<&'static str> {
        [
            (Self::ASYNC, "async"),
            (Self::TRAPPABLE, "trappable"),
            (Self::STORE, "store"),
            (Self::TRACING, "tracing"),
            (Self::VERBOSE_TRACING, "verbose_tracing"),
            (Self::EXACT, "exact"),
        ]
        .into_iter()
        .filter(|(flag, _)| self.contains(*flag))
        .map(|(_, name)| name)
        .collect()
    }
}

impl BitOr for FunctionFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FunctionFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
