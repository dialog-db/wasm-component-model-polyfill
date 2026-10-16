// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The arguments of one call of a host function registration.

use core::any::Any;

use crate::value::Val;

/// How the arguments of one call reach a registration's body.
pub enum HostArgs {
    /// One [`Val`] per parameter, for an untyped registration.
    Vals(Vec<Val>),
    /// The registration's own Rust parameter tuple, lifted straight
    /// out of the guest, for a typed one.
    Typed(Box<dyn Any + Send>),
}
