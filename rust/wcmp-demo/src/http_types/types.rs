// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The resource types of `wasi:http/types`.

use wcmp::ResourceTypeId;

/// The resource types [`define`] registered, which a store's
/// [`HttpTable`] needs before the host makes a handle.
///
/// [`define`]: super::define
/// [`HttpTable`]: super::HttpTable
#[derive(Debug, Clone, Copy)]
pub struct Types {
    /// The resource type of `fields`.
    pub fields: ResourceTypeId,
    /// The resource type of `request`.
    pub request: ResourceTypeId,
    /// The resource type of `response`.
    pub response: ResourceTypeId,
}

impl Types {
    /// The resource type of `request`, for a handle the host makes with
    /// `Store::resource_new` before a driver runs.
    pub fn request(&self) -> ResourceTypeId {
        self.request
    }
}
