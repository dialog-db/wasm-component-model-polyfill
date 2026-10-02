// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Routes: URL patterns that the service worker answers with a
//! component that it compiles from Zena source.
//!
//! A [`Router`] keeps the routes in the order of definition and matches
//! each request against them in that order. It compiles a route on the
//! first request that matches it, and keeps the instance until the
//! browser stops the worker, which drops every instance with it.
//!
//! A route component exports `wasi:http/handler@0.3.0`. The router hands
//! it the request as a `wasi:http` request, and reads back the response
//! it answers, its body included.
//!
//! A route that does not compile answers each request with 500 and the
//! diagnostics. A route that traps answers that request with 500, and
//! the router drops its instance, so the next request compiles it
//! again.

mod http_request;
mod http_response;
mod route_event;
mod route_host;
mod route_status;
mod router;

pub use http_request::HttpRequest;
pub use http_response::HttpResponse;
pub use route_event::RouteEvent;
pub use route_host::RouteHost;
pub use route_status::RouteStatus;
pub use router::Router;

/// Whether `path` matches `pattern`, segment by segment, where a segment
/// of the pattern that starts with `:` matches any one segment.
pub fn matches(pattern: &str, path: &str) -> bool {
    let want: Vec<&str> = pattern.split('/').collect();
    let have: Vec<&str> = path.split('/').collect();
    want.len() == have.len()
        && want
            .iter()
            .zip(&have)
            .all(|(want, have)| want.starts_with(':') || want == have)
}
