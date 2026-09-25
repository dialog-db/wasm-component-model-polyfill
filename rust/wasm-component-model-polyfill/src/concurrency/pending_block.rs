//! A blocking built-in a suspended thread waits in.

use super::block_step::BlockStep;
use super::readiness::Readiness;

/// A blocking built-in a thread suspended in, under a provider, from
/// the try part that began the wait to the finish part that ends it.
///
/// The shim of the built-in calls the try part again each time the
/// thread resumes, and the try part then only asks whether the
/// condition holds: the first part of the built-in ran once, when
/// the wait began.
pub struct PendingBlock<T: 'static> {
    /// The condition the thread waits on.
    pub readiness: Readiness,
    /// The condition the thread's record held before, which the wait
    /// puts back when it ends.
    pub previous: Option<Readiness>,
    /// The step whose finish part computes the built-in's results.
    /// Always the waiting form.
    pub step: BlockStep<T>,
}
