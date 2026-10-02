// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The capability lexicon, and the set of capabilities a backend declares.

mod capabilities;
#[allow(clippy::module_inception)]
mod capability;

pub use capabilities::Capabilities;
pub use capability::Capability;
