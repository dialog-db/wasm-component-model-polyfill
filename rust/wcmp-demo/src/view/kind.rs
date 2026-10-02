// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What one node of a view is.

use super::Property;

/// What one node of a view is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// An element, with its attributes, its properties, and the
    /// handler each DOM event calls.
    Element {
        /// The tag name.
        tag: String,
        /// Each attribute, in order.
        attributes: Vec<(String, String)>,
        /// Each property, in order.
        properties: Vec<(String, Property)>,
        /// Each DOM event name, with the name of the handler it calls.
        events: Vec<(String, String)>,
    },
    /// Text.
    Text(String),
}
