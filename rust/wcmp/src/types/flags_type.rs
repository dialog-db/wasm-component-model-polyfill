// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The structural shape of a `flags` value type.

/// A bit-set whose discriminants are addressed by name.
///
/// Two flags types are structurally equal when their flag names
/// appear in the same order.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FlagsType {
    names: Vec<String>,
}

impl FlagsType {
    /// Construct a flags type from an ordered list of flag names.
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    /// The flag names, in declaration order.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}
