//! Host suspension: a resumable call, and a call that waits.

mod resumable_call;
mod suspended_call;

pub use resumable_call::ResumableCall;
pub use suspended_call::SuspendedCall;
