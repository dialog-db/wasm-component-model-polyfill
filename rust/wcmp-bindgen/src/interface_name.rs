// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where an interface's code is.

/// Where an interface's code is.
#[derive(Clone)]
pub enum InterfaceName {
    /// The macro writes the interface at this path of modules from the
    /// root of the invocation.
    Path(Vec<String>),
}
