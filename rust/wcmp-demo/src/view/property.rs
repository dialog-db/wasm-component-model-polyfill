// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A property a view sets on a DOM node.

/// A property the view sets on a DOM node, rather than an attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Property {
    /// A text property, such as `value`.
    Text(String),
    /// A flag, such as `checked`.
    Flag(bool),
}
