// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A request, as the router takes it.

/// A request, as the router takes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpRequest {
    /// The method, upper case.
    pub method: String,
    /// The path and query, such as `/api/todos?filter=all`.
    pub path_with_query: String,
    /// Each header, in order.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}
