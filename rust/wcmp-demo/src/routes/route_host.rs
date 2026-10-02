// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The store data of a route component.

use std::sync::Arc;

use crate::http_types::{HttpHost, HttpTable};
use crate::model::Storage;
use crate::model_host::{Model, ModelHost};

/// The store data of a route component.
pub struct RouteHost<S: Storage + 'static> {
    /// The HTTP resources of the route's component.
    pub http: HttpTable,
    /// The todo model the route reaches.
    pub model: Arc<Model<S>>,
}

impl<S: Storage + 'static> HttpHost for RouteHost<S> {
    fn http(&mut self) -> &mut HttpTable {
        &mut self.http
    }
}

impl<S: Storage + 'static> ModelHost for RouteHost<S> {
    type Storage = S;

    fn model(&self) -> Arc<Model<S>> {
        self.model.clone()
    }
}
