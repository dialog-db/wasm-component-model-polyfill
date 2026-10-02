// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One error context: a debug message the guests that hold it share.

/// The record of one error context.
///
/// The store keeps one record per error context, whichever instances
/// hold a handle to it. The record holds the debug message exactly
/// as `error-context.new` read it, and the count of the guest
/// handles that name it. `error-context.new` creates the record with
/// a count of one, a crossing into another instance adds one, and
/// `error-context.drop` subtracts one. A lift to the host marks the
/// record host-held. The record leaves the store when the count
/// reaches zero, unless the host holds it: the host cannot drop an
/// error context, so a host-held record stays until the store drops.
pub struct ErrorContextRecord {
    /// The debug message, as the guest wrote it.
    pub debug_message: String,
    /// How many guest handles name the record.
    pub handle_count: u32,
    /// Whether a lift has handed the record to the host.
    pub host_held: bool,
}

impl ErrorContextRecord {
    /// Construct the record `error-context.new` creates: one handle
    /// names it, and the host does not hold it.
    pub fn new(debug_message: String) -> Self {
        Self {
            debug_message,
            handle_count: 1,
            host_held: false,
        }
    }
}
