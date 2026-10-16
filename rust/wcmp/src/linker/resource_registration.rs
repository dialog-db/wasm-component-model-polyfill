// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One host resource type registered on a [`LinkerInstance`].
//!
//! [`LinkerInstance`]: super::LinkerInstance

use std::sync::Arc;

use crate::error::Result;
use crate::store::StoreContext;
use crate::types::ResourceType;

/// One host resource type registered on a [`crate::LinkerInstance`]
/// under a name: its identity and its destructor.
///
/// The identity is the host's, as in Wasmtime. Two registrations with
/// the same identity, under two names or two interfaces, are one
/// resource type: a handle that one of them lets a guest have lowers
/// through the other, and the store keeps one table and one destructor
/// for it.
pub struct ResourceRegistration<T: 'static> {
    /// The identity of the resource type.
    pub ty: ResourceType,
    /// The destructor that runs when a guest drops the last handle.
    pub destructor: Arc<DestructorBody<T>>,
}

/// The closure a [`ResourceRegistration`] holds.
///
/// The closure takes a context for the store and the resource's rep,
/// and runs synchronously when the guest drops an owned handle. An
/// error fails the drop with that error.
pub type DestructorBody<T> =
    dyn for<'a> Fn(StoreContext<'a, T>, u32) -> Result<()> + Send + Sync + 'static;

impl<T: 'static> Clone for ResourceRegistration<T> {
    fn clone(&self) -> Self {
        Self {
            ty: self.ty,
            destructor: self.destructor.clone(),
        }
    }
}
