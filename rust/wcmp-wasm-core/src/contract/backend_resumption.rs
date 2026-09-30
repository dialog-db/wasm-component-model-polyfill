//! A resumable call that runs, as its backend holds it.

use core::any::Any;

use crate::contract::MaybeSend;

/// A resumable call that runs, started or resumed, as the backend that
/// runs it holds it, until a wait sees its next stop.
///
/// The resumption does not hold its store, and it does not wait for itself.
/// The store waits for it, through [`BackendStore::stop_resumption`], so the
/// backend reaches the resumption and the store each as its own concrete
/// type.
///
/// Where it drops before a wait saw its stop, the call it runs stops the
/// next time it would reach its store, and never runs again, unless the
/// store dropped first.
///
/// [`BackendStore::stop_resumption`]: crate::backend::BackendStore::stop_resumption
pub trait BackendResumption: MaybeSend + 'static {
    /// The resumption as a value the store that waits for it downcasts to
    /// the backend's own type.
    ///
    /// A backend implements it as `self`.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
