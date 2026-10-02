// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A network response from Chrome's performance log.

/// A network response from Chrome's performance log.
#[derive(Debug, Clone)]
pub struct Response {
    /// The URL.
    pub url: String,
    /// The status.
    pub status: u16,
    /// Each header, by its lower-case name.
    pub headers: Vec<(String, String)>,
    /// Whether the service worker answered the request.
    pub from_service_worker: bool,
}

impl Response {
    /// The header `name`, if the response has it.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}
