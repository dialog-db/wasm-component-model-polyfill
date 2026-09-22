//! Where a task's failure goes when the task ends with one.

use crate::error::Error;

use super::wake_slot::WakeSlot;

/// The channel the call that started a task watches for that task's
/// failure.
///
/// A task resolves with a result and fails with an error, and the
/// two travel separately: the result goes through the task's
/// [`ResultChannel`](super::ResultChannel) and the error goes
/// through this. The call watches both and reads this one first, so
/// a task that resolved and then failed — on the borrows the guest
/// still owed, or on a host call of its own that never returned —
/// fails its call rather than answering it.
///
/// Both sides hold the channel. The call fills it from the item or
/// the turn that ended the task, with the call's future nowhere on
/// the stack, and the wake beside the value is what brings a future
/// a host combinator owns back to read it.
///
/// A task the store ends with a failure and no channel has no call
/// to report to. Its error ends the turn instead and reaches
/// whichever driver was polling, which is the rule
/// [`Func::call`](crate::Func::call) and
/// [`Store::run_concurrent`](crate::Store::run_concurrent) state.
pub type FailureChannel = WakeSlot<Error>;
