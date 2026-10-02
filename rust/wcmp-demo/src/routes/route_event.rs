// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the router tells the page about a route.

use super::RouteStatus;

/// What the router tells the page about a route.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteEvent {
    /// The router compiled the route: its status afterwards.
    Compiled(RouteStatus),
    /// A request trapped the route's instance.
    Trapped(RouteStatus),
}
