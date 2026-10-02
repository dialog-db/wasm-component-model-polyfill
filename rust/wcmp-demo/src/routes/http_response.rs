// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A response, as the router answers it.

/// A response, as the router answers it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status code.
    pub status: u16,
    /// Each header, in order.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response with `status` and a text body.
    pub fn text(status: u16, body: &str) -> Self {
        HttpResponse {
            status,
            headers: vec![(
                "content-type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body: body.as_bytes().to_vec(),
        }
    }
}
