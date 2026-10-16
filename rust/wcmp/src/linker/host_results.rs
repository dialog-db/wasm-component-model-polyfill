// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The results of one call of a host function registration.

use crate::value::Val;

use super::typed_results::TypedResults;

/// What one call of a registration answers with, ready to cross into
/// the guest.
pub enum HostResults<T: 'static> {
    /// One [`Val`] when the type declares a result and none otherwise,
    /// from an untyped registration.
    Vals(Vec<Val>),
    /// A typed registration's result tuple, which lowers itself
    /// straight into the guest.
    Typed(Box<dyn TypedResults<T>>),
}
