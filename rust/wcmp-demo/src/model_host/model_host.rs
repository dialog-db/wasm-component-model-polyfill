// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The store data of a route component.

use std::sync::Arc;

use super::Model;
use crate::model::Storage;

/// The store data of a route component: it reaches the model.
pub trait ModelHost: 'static {
    /// The storage the model keeps the list in.
    type Storage: Storage + 'static;

    /// The model.
    fn model(&self) -> Arc<Model<Self::Storage>>;
}
