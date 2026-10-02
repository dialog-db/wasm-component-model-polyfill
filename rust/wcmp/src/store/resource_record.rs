// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a store knew about one resource type at one moment.

use crate::resource::ResourceTypeId;
use crate::types::ResourceType;

/// What a store knew about one resource type at one moment: whether
/// a destructor was registered for it, and the name it rendered for
/// it, with who taught that name.
///
/// An instantiation takes one of these for every resource type it is
/// about to register, and hands them back if the plan fails, so that
/// a failed instantiation leaves the store's registrations as it
/// found them. The two names are the store's two tiers, and at most
/// one of them is ever set: a label a component taught outranks a
/// label the linker's sweep left as a fallback, and the record has
/// to put back the tier as well as the label, or a later component
/// would find a taught name where the store held a fallback.
///
/// A record is taken once per resource type, before the
/// instantiation registers anything for it. A second record of the
/// same type would see the instantiation's own registration and put
/// that back instead of what preceded it.
pub struct ResourceRecord {
    /// The resource type the record is about.
    pub type_id: ResourceTypeId,
    /// Whether the store had a destructor registered for it.
    pub destructor: bool,
    /// The label a component instantiated into the store had taught
    /// for it, if one had.
    pub taught_name: Option<ResourceType>,
    /// The label the store had to fall back on for it while no
    /// component had named it, if it held one.
    pub fallback_name: Option<ResourceType>,
}
