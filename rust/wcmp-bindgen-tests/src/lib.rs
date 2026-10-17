// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tests of `wcmp::bindgen!`, under `tests/`.
//!
//! The crate has no code of its own. It is the crate the tests invoke the
//! macro from, so the macro reads `wit/` beside its manifest when an
//! invocation names no path, and every relative path in an invocation
//! resolves against its directory.
