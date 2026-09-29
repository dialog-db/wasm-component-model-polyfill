//! A resumable call that waits.

use core::fmt;

use crate::call::ResumableCall;
use crate::capability::Capability;
use crate::checks;
use crate::contract::BackendSuspendedCall;
use crate::error::Result;
use crate::internal::StoreContextMutInternal;
use crate::store::{AsContextMut, StoreId};
use crate::values::Val;

/// A resumable call that waits in a suspending host function.
///
/// The handle does not hold its store. It borrows the store only while it
/// resumes. Any number of calls can wait at once in one store, and the host
/// can resume them in any order. When the store drops, its waiting calls
/// drop without a resumption.
pub struct SuspendedCall {
    store: StoreId,
    inner: Box<dyn BackendSuspendedCall>,
}

impl SuspendedCall {
    /// The waiting call `inner` of the store `store`.
    ///
    /// A backend makes one when a suspending host function answers "not
    /// yet".
    pub fn new(store: StoreId, inner: Box<dyn BackendSuspendedCall>) -> Self {
        Self { store, inner }
    }

    /// Resumes the call with `import_results`, the results of the
    /// suspending host function, and runs it to its next suspension or its
    /// end. At its end, the results of the call are in `results`.
    ///
    /// The resumption is asynchronous, because the browser runs the rest of
    /// the call on a microtask after the resumption starts. `store` is the
    /// store the call waits in, or the [`Caller`](crate::Caller) of a host
    /// function that runs in it. A call resumed with a store other than its
    /// own is [`Error::WrongStore`](crate::Error::WrongStore).
    ///
    /// The backend must declare
    /// [`host_suspension`](Capability::HostSuspension). Where it does not,
    /// this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub async fn resume(
        self,
        mut store: impl AsContextMut,
        import_results: &[Val],
        results: &mut [Val],
    ) -> Result<ResumableCall> {
        let mut store = store.as_context_mut();
        store
            .engine()
            .capabilities()
            .require(Capability::HostSuspension)?;
        let backend = store.backend_mut();
        if backend.data().id() != self.store {
            return Err(crate::Error::WrongStore);
        }
        checks::values_in_store(backend, import_results)?;
        backend
            .resume_call(self.inner, import_results, results)
            .await
    }
}

impl fmt::Debug for SuspendedCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuspendedCall")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}
