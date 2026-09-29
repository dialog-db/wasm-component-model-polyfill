//! How a resumable call ends.

use crate::call::SuspendedCall;

/// How a resumable call, or one resumption of it, ends.
///
/// The type is non-exhaustive: a later design adds a third way, a call
/// that runs out of fuel. So a host matches it with a wildcard arm:
///
/// ```
/// use wcmp_wasm_core::ResumableCall;
///
/// fn describe(outcome: &ResumableCall) -> &'static str {
///     match outcome {
///         ResumableCall::Finished => "finished",
///         ResumableCall::Suspended(_) => "suspended",
///         _ => "something a later design adds",
///     }
/// }
/// # let _ = describe;
/// ```
///
/// A match without the wildcard arm does not compile outside this crate:
///
/// ```compile_fail
/// use wcmp_wasm_core::ResumableCall;
///
/// fn describe(outcome: &ResumableCall) -> &'static str {
///     match outcome {
///         ResumableCall::Finished => "finished",
///         ResumableCall::Suspended(_) => "suspended",
///     }
/// }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ResumableCall {
    /// The call ran to its end. Its results are in the slice of results the
    /// call or the resumption was given.
    Finished,
    /// A suspending host function answered "not yet". The call waits until
    /// the host resumes it.
    Suspended(SuspendedCall),
}
