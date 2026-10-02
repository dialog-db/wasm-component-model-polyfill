// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The status word a lowered call of a host function returns.

use super::subtask_state::SubtaskState;

/// The status word a lowered call of a host function returns to the
/// guest: the subtask state in the low four bits and the subtask's
/// index in the caller instance's handle table above them.
///
/// A call whose future resolved on the first poll returns its result
/// at once and leaves no subtask behind, so its status is the
/// returned state alone and the index bits are zero. A call that is
/// still running returns the started state and the index of the
/// entry the guest waits on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallStatus(u32);

impl CallStatus {
    /// How many low bits the state occupies. The reference packs the
    /// index above them, and a guest reads the word this way.
    const STATE_BITS: u32 = 4;

    /// The status of a call that returned its result at once.
    pub fn returned() -> Self {
        Self(SubtaskState::Returned.value())
    }

    /// The status of a call that is still running, waited on through
    /// the handle-table entry at `handle_index`.
    pub fn started(handle_index: u32) -> Self {
        Self::in_progress(SubtaskState::Started, handle_index)
    }

    /// The status of a call that has not resolved, in the state it
    /// is in, waited on through the handle-table entry at
    /// `handle_index`.
    ///
    /// A call into another component can still be at its callee's
    /// entry gate when the lower returns, which is the starting
    /// state; a call into a host function is past it either way, so
    /// [`started`](Self::started) is the one shape that reaches.
    pub fn in_progress(state: SubtaskState, handle_index: u32) -> Self {
        Self(state.value() | (handle_index << Self::STATE_BITS))
    }

    /// The subtask state in the low four bits.
    pub fn state(self) -> u32 {
        self.0 & ((1 << Self::STATE_BITS) - 1)
    }

    /// The subtask's index in the caller instance's handle table, or
    /// `None` for a call that left no subtask behind.
    pub fn subtask_index(self) -> Option<u32> {
        if self.state() == SubtaskState::Returned.value() {
            return None;
        }
        Some(self.0 >> Self::STATE_BITS)
    }

    /// The word itself, as the guest reads it.
    pub fn value(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    async fn it_packs_the_state_below_the_subtask_index() {
        let started = CallStatus::started(3);
        assert_eq!(started.value(), 1 | (3 << 4));
        assert_eq!(started.state(), SubtaskState::Started.value());
        assert_eq!(started.subtask_index(), Some(3));
    }

    #[wcmp_macros::test]
    async fn it_carries_no_subtask_when_the_call_returned_at_once() {
        let returned = CallStatus::returned();
        assert_eq!(returned.value(), SubtaskState::Returned.value());
        assert_eq!(returned.subtask_index(), None);
    }
}
