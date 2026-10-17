// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What an import's traits have beyond its functions.

use wit_parser::TypeId;

/// What the traits of a set of imported functions have beyond the
/// functions.
#[derive(Default)]
pub struct TraitExtras<'a> {
    /// The error types whose `convert_*` methods `Host` has.
    pub converts: &'a [(String, TypeId)],
    /// The WIT names of the resources whose traits the two traits
    /// extend, as an interface's `Host` extends its resources' `HostR`.
    pub resources: &'a [String],
    /// The WIT name of the resource whose destructor the traits have,
    /// for a resource's own traits.
    pub drop: Option<&'a str>,
}
