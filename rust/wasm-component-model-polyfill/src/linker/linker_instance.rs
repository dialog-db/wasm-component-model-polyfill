//! A borrowed view onto one interface's registration in a [`Linker`].
//!
//! [`Linker`]: super::Linker

use core::marker::PhantomData;

use super::registration::InstanceRegistration;

/// A borrowed view onto one interface's worth of host items inside a
/// [`Linker`].
///
/// `LinkerInstance` is the addressing surface for "the host items
/// that satisfy *this* interface": it is obtained from
/// [`Linker::instance`] and carries the back-reference the linker
/// uses to look up or insert a per-interface registration entry.
///
/// At present `LinkerInstance` exposes no methods of its own — the
/// type exists so identifier resolution has a concrete candidate to
/// match against. Later work attaches the host-function and host-
/// resource registration modes to this type.
///
/// [`Linker`]: super::Linker
/// [`Linker::instance`]: super::Linker::instance
pub struct LinkerInstance<'a, T> {
    /// The owned registration this view borrows. Held mutably so
    /// later registration methods can populate it without further
    /// linker access. Workspace-internal: this field is not
    /// re-exported by `lib.rs` and never reaches downstream
    /// consumers.
    pub registration: &'a mut InstanceRegistration<T>,
    /// `T` participates only as the host-data type the registration
    /// carries; capture invariance explicitly so the parameter does
    /// not appear unused.
    _phantom: PhantomData<fn(T) -> T>,
}

impl<'a, T> LinkerInstance<'a, T> {
    /// Construct a borrowed view onto the given registration.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn new(registration: &'a mut InstanceRegistration<T>) -> Self {
        Self {
            registration,
            _phantom: PhantomData,
        }
    }
}
