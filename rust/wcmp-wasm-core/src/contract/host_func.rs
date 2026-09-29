//! The body of a host function, as a backend calls it.

use core::fmt;
use core::task::Poll;
use std::sync::Arc;

use crate::contract::BackendStore;
use crate::values::Val;

/// The erased body every host function shares: the store the guest runs
/// in, the arguments, and a slot for each result.
type Body =
    dyn Fn(&mut dyn BackendStore, &[Val], &mut [Val]) -> anyhow::Result<Poll<()>> + Send + Sync;

/// The body of a host function, as a backend calls it.
///
/// The engine makes one from the host's closure in
/// [`Func::new`](crate::Func::new) or
/// [`Func::new_suspending`](crate::Func::new_suspending), and hands it to
/// [`BackendStore::func_new`]. The backend calls it each time a guest calls
/// the function, with the store the guest runs in. A backend holds four
/// rules for it:
///
/// - The body can be entered again while an earlier call of it runs, at any
///   depth.
/// - Each call has its own arguments and its own results. No call shares a
///   buffer with another call.
/// - An error from the body traps the guest with
///   [`TrapKind::Host`](crate::TrapKind::Host), carrying the error
///   unchanged. No guest can catch the trap.
/// - A body that is not suspending always answers [`Poll::Ready`]. A
///   suspending body can answer [`Poll::Pending`]: "not yet". Inside a
///   resumable call whose frames between its start and the host function
///   are all WebAssembly, the backend then suspends the call. Anywhere else,
///   the call traps.
#[derive(Clone)]
pub struct HostFunc {
    body: Arc<Body>,
    suspending: bool,
}

impl HostFunc {
    /// A host function whose body always answers with its results.
    pub fn new(
        body: impl Fn(&mut dyn BackendStore, &[Val], &mut [Val]) -> anyhow::Result<()>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            body: Arc::new(move |store, params, results| {
                body(store, params, results).map(|()| Poll::Ready(()))
            }),
            suspending: false,
        }
    }

    /// A suspending host function, whose body can answer
    /// [`Poll::Pending`] in place of its results.
    pub fn suspending(
        body: impl Fn(&mut dyn BackendStore, &[Val], &mut [Val]) -> anyhow::Result<Poll<()>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            body: Arc::new(body),
            suspending: true,
        }
    }

    /// Whether the body can answer [`Poll::Pending`].
    pub fn is_suspending(&self) -> bool {
        self.suspending
    }

    /// Runs the body with `params`, in the store the guest runs in.
    ///
    /// On [`Poll::Ready`], the results are in `results`.
    pub fn call(
        &self,
        store: &mut dyn BackendStore,
        params: &[Val],
        results: &mut [Val],
    ) -> anyhow::Result<Poll<()>> {
        (self.body)(store, params, results)
    }
}

impl fmt::Debug for HostFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostFunc")
            .field("suspending", &self.suspending)
            .finish_non_exhaustive()
    }
}
