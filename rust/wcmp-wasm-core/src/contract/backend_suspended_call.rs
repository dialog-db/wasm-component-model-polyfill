//! A resumable call that a backend set aside.

use core::any::Any;

use crate::contract::MaybeSend;

/// A resumable call that waits in a suspending host function, as the
/// backend that suspended it holds it.
///
/// The call does not hold its store, and it does not resume itself. The
/// store resumes it, through [`BackendStore::resume_call`], so the backend
/// reaches the call and the store each as its own concrete type. The store
/// that resumes a call can be the store itself, or the context a host
/// function received while a guest runs in the store: each is a type of the
/// backend, and each knows how to reach the state the call needs.
///
/// Any number of calls can wait at once in one store, and the host can
/// resume them in any order. When a store drops, its waiting calls drop
/// without a resumption. A resumption that started before the drop runs to
/// its next suspension or its end first, and the backend keeps the state of
/// the store alive until then.
///
/// [`BackendStore::resume_call`]: crate::backend::BackendStore::resume_call
pub trait BackendSuspendedCall: MaybeSend + 'static {
    /// The call as a value the store that resumes it downcasts to the
    /// backend's own type.
    ///
    /// A backend implements it as `self`.
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}
