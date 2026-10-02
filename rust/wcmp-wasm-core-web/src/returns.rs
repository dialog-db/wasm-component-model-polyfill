// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How the host reads the results of a call into a guest.

use wcmp_wasm_core::FuncType;

/// How the host reads the results of a call into a guest: the type of the
/// function, where the backend knows it, and whether the call went through
/// a carrier.
///
/// A resumable call keeps it from its start to its end, since its results
/// arrive only then.
#[derive(Clone, Debug)]
pub struct Returns {
    /// The type of the function, or `None` for a function reference that a
    /// guest handed out, whose results the slots of the call tell.
    pub ty: Option<FuncType>,
    /// Whether the call went through a carrier, which gives the results in
    /// its own form.
    pub carried: bool,
}
