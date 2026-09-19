//! How the caller of a prepared call takes its result.

/// How many flat results the canonical ABI passes without a return
/// pointer.
const MAX_FLAT_RESULTS: u32 = 1;

/// How the caller of a call between two components takes the
/// callee's result.
///
/// The prepare intrinsic of a fused adapter carries this as one
/// number, which the reference implementation calls
/// `result_count_or_max_if_async`. A synchronous lower passes the
/// count of its own flat results; an asynchronous lower passes one
/// of two sentinels, for a call that produces a result and one that
/// does not.
///
/// The kind decides two things. It says whether the caller passed a
/// return pointer as the last of its flat arguments, which the
/// return function needs appended to the results it is called with.
/// And it says whether the call's flat results go back to the caller
/// as the start intrinsic returns, which is the synchronous lower's
/// case alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallerKind {
    /// A `canon lower` without `async`: the caller receives the
    /// callee's flat results when the call returns. `flat_results`
    /// is how many of them there are, counted with no limit, so a
    /// count above the one flat result the canonical ABI allows is
    /// the case where the caller passed a return pointer instead.
    Sync {
        /// How many flat results the caller's lowered signature has.
        flat_results: u32,
    },
    /// A `canon lower` with `async`: the caller receives a status
    /// word, and the result, when the call has one, is written
    /// through the return pointer the caller passed.
    Async {
        /// Whether the call produces a result, and so whether the
        /// caller passed a return pointer.
        has_result: bool,
    },
}

impl CallerKind {
    /// Whether the caller passed a return pointer as the last of its
    /// flat arguments. The canonical ABI allows one flat result, so
    /// a synchronous caller with more than one passes a pointer
    /// instead, and an asynchronous caller always does when the call
    /// has a result.
    pub fn has_return_pointer(self) -> bool {
        match self {
            Self::Sync { flat_results } => flat_results > MAX_FLAT_RESULTS,
            Self::Async { has_result } => has_result,
        }
    }
}
