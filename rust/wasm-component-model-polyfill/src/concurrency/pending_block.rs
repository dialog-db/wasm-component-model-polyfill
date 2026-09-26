//! A blocking built-in a suspended thread waits in.

use crate::error::Result;

use super::block_step::BlockStep;
use super::readiness::Readiness;

/// A blocking built-in a thread suspended in, under a provider, from
/// the try part that began the wait to the finish part that ends it.
///
/// The shim of the built-in calls the try part again each time the
/// thread resumes, and the try part then only asks whether the
/// condition holds: the first part of the built-in ran once, when
/// the wait began.
///
/// A built-in whose thread suspended so that the scheduler could run
/// a plan for it, under a provider that resumes a thread only where
/// the store runs no guest code, waits on [`Readiness::Planned`]. The
/// scheduler leaves the plan's outcome here before it resumes the
/// thread, and the retry hands it to the finish part: what the wait
/// the plan ran ended with, or the failure the plan's work ended with
/// where the work would have failed the built-in's first part.
pub struct PendingBlock<T: 'static> {
    /// The condition the thread waits on.
    pub readiness: Readiness,
    /// The condition the thread's record held before, which the wait
    /// puts back when it ends.
    pub previous: Option<Readiness>,
    /// The step whose finish part computes the built-in's results.
    pub step: BlockStep<T>,
    /// What the plan the built-in left ended with, once it is done.
    /// `None` for a built-in whose condition the finish part is
    /// handed as having held.
    pub waited: Option<Result<()>>,
}

impl<T: 'static> PendingBlock<T> {
    /// A built-in that waits on `readiness`, with `previous` the
    /// condition the thread's record held before, and `step` its
    /// finish.
    pub fn new(readiness: Readiness, previous: Option<Readiness>, step: BlockStep<T>) -> Self {
        Self {
            readiness,
            previous,
            step,
            waited: None,
        }
    }
}
