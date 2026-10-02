// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where a task's result goes when the task resolves.

use crate::value::Val;

use super::wake_slot::WakeSlot;

/// The channel a caller that is not on the stack watches for a
/// task's result.
///
/// The slot is empty until the task resolves. What it holds then is
/// the result itself, absent for a function that declares none. Both
/// sides hold the channel: the task fills it as it resolves, which
/// wakes whoever waits on it, and the caller's future takes the value
/// out.
pub type ResultChannel = WakeSlot<Option<Val>>;

/// Where a task's result goes when the task resolves.
///
/// A task whose caller is on the stack — every call of the
/// synchronous baseline — leaves its result in the record, and the
/// caller takes it as the call returns. A task whose caller is not
/// on the stack is given the caller's channel instead, and the
/// result is sent through it as the task resolves.
#[derive(Clone, Debug)]
pub enum TaskResult {
    /// The task has not resolved yet and no caller is waiting on a
    /// channel.
    Pending,
    /// The result the task returned, held in the record until its
    /// caller takes it. The inner `Option` is absent for a function
    /// that declares no result.
    Returned(Option<Val>),
    /// The channel of a caller that is not on the stack. The slot is
    /// filled once, with the result the task returned. A host call
    /// into an asynchronous export takes this shape: the task
    /// outlives the call, so the call's driver watches the channel
    /// rather than the record.
    Channel(ResultChannel),
}
