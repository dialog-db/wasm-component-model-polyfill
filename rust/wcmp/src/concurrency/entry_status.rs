// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where a thread entry stopped when a provider handed control back.

use crate::runtime_layer::Val as RuntimeVal;

/// Where a thread entry stopped when a provider's start or resume
/// handed control back to its caller.
///
/// A provider reports where the entry stopped at the moment it
/// stops. The results of an entry that finished come with the answer:
/// the switch module's entry wrapper handed them to the host before it
/// returned, whether or not the entry suspended on the way.
///
/// A provider that runs a thread on after the call that asked for it
/// returns answers [`Running`](Self::Running) instead, and the caller
/// learns where the thread stopped later, from the provider's
/// `poll_stop`. The host-suspension provider answers it for every resume, since
/// a resumed stack runs on a microtask, and for a start whose thread
/// failed before it first suspended, since the browser hands over the
/// failure on a microtask too.
#[derive(Clone, Debug)]
pub enum EntryStatus {
    /// The entry returned these core results, and its thread is
    /// gone from the provider.
    Finished(Vec<RuntimeVal>),
    /// The entry suspended in a shim, and the provider keeps its
    /// thread until a resume names it.
    Suspended,
    /// The thread runs on, or has failed, after the call returned,
    /// and the provider's `poll_stop` answers where it stopped.
    // Only the host-suspension provider answers it, and it exists in the browser
    // alone.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Running,
}
