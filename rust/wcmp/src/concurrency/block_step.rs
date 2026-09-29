//! What the first part of a blocking built-in found.

use wasm_runtime_layer::Val as RuntimeVal;

use crate::error::Result;
use crate::store::StoreContext;

use super::readiness::Readiness;

/// The boxed finish part of a blocking built-in, with the `Send`
/// bound the native target puts on everything a store holds.
#[cfg(not(target_arch = "wasm32"))]
type BoxedFinish<T> = Box<
    dyn FnOnce(&mut StoreContext<'_, T>, Result<()>) -> anyhow::Result<Vec<RuntimeVal>>
        + Send
        + 'static,
>;

/// The boxed finish part of a blocking built-in. The browser drops
/// the `Send` bound, as it does for the action of an item.
#[cfg(target_arch = "wasm32")]
type BoxedFinish<T> = Box<
    dyn FnOnce(&mut StoreContext<'_, T>, Result<()>) -> anyhow::Result<Vec<RuntimeVal>> + 'static,
>;

/// What the first part of a blocking built-in found.
///
/// A blocking built-in splits into two parts, as the reference's
/// `Thread.wait_until` does. The first part does what the built-in
/// does before it can wait: it checks its arguments, starts a copy,
/// starts a callee, or starts a host future. It then answers either
/// that the built-in is done, with the results the guest reads, or
/// the readiness condition the built-in waits on and the finish part
/// that computes its results once the condition holds.
///
/// The finish part is handed how the wait went. A wait that failed —
/// the store could not serve it, or it unwound — hands the finish
/// part the failure, and the finish part gives back what the first
/// part took and answers the trap the guest sees. A wait that ended
/// with the condition true hands it `Ok(())`.
///
/// The first part never waits itself, so whoever calls it decides
/// how the thread waits: through the nested turn of the suspend seam
/// where the thread runs on the real stack, or by suspending the
/// thread's own stack in the switch module's shim under a provider.
pub enum BlockStep<T: 'static> {
    /// The built-in is done, and these are its results.
    Ready(Vec<RuntimeVal>),
    /// The built-in waits until the condition holds, and the finish
    /// part then computes its results.
    Wait {
        /// The condition the thread waits on.
        readiness: Readiness,
        /// The finish part.
        finish: BoxedFinish<T>,
    },
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: 'static> BlockStep<T> {
    /// Wait until `readiness` holds, and compute the results with
    /// `finish`.
    pub fn wait(
        readiness: Readiness,
        finish: impl FnOnce(&mut StoreContext<'_, T>, Result<()>) -> anyhow::Result<Vec<RuntimeVal>>
        + Send
        + 'static,
    ) -> Self {
        Self::Wait {
            readiness,
            finish: Box::new(finish),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> BlockStep<T> {
    /// Wait until `readiness` holds, and compute the results with
    /// `finish`.
    pub fn wait(
        readiness: Readiness,
        finish: impl FnOnce(&mut StoreContext<'_, T>, Result<()>) -> anyhow::Result<Vec<RuntimeVal>>
        + 'static,
    ) -> Self {
        Self::Wait {
            readiness,
            finish: Box::new(finish),
        }
    }
}

impl<T: 'static> BlockStep<T> {
    /// The condition the built-in waits on, or `None` when it is
    /// done.
    pub fn readiness(&self) -> Option<Readiness> {
        match self {
            Self::Ready(_) => None,
            Self::Wait { readiness, .. } => Some(*readiness),
        }
    }

    /// The built-in's results: the ones it was done with, or what its
    /// finish part computes once the wait went as `waited` says.
    pub fn finish(
        self,
        store: &mut StoreContext<'_, T>,
        waited: Result<()>,
    ) -> anyhow::Result<Vec<RuntimeVal>> {
        match self {
            Self::Ready(values) => Ok(values),
            Self::Wait { finish, .. } => finish(store, waited),
        }
    }
}
