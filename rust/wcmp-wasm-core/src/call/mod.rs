// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Host suspension: a resumable call, a call that waits, and a call that
//! runs.

mod resumable_call;
mod resumption;
mod suspended_call;

pub use resumable_call::ResumableCall;
pub use resumption::Resumption;
pub use suspended_call::SuspendedCall;
