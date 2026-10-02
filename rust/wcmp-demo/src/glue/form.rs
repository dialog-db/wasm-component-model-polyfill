// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The two forms of an element's definition.

/// The two forms of an element's definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// A class that extends `Element`, by its name.
    Class(String),
    /// Functions: `render`, and optionally `on`, `styles`, and
    /// `attributes`.
    Functions {
        /// Whether the source exports `on`.
        on: bool,
        /// Whether the source exports `styles`.
        styles: bool,
        /// Whether the source exports `attributes`.
        attributes: bool,
    },
}
