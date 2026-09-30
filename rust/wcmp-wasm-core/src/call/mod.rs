//! Host suspension: a resumable call, a call that waits, and a call that
//! runs.

mod resumable_call;
mod resumption;
mod suspended_call;

pub use resumable_call::ResumableCall;
pub use resumption::Resumption;
pub use suspended_call::SuspendedCall;
