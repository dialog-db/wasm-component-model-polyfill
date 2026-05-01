//! The owned, per-interface state a [`Linker`] holds for each
//! registered [`LinkerInstance`].
//!
//! In the present surface this state is empty: a registration exists
//! to satisfy identifier resolution, not to carry host items. Later
//! work attaches host functions and host resources here, alongside
//! whatever bookkeeping their canonical-ABI handling requires.
//!
//! [`Linker`]: super::Linker
//! [`LinkerInstance`]: super::LinkerInstance

use core::marker::PhantomData;

/// The polyfill's owned, per-interface registration entry.
///
/// `T` is the [`Store`]'s host-data type; the variance is captured
/// invariantly through `fn(T) -> T` so this entry composes with the
/// invariance the rest of the polyfill imposes on `T`.
///
/// [`Store`]: crate::Store
pub struct InstanceRegistration<T> {
    _phantom: PhantomData<fn(T) -> T>,
}

impl<T> InstanceRegistration<T> {
    /// Construct an empty registration.
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for InstanceRegistration<T> {
    fn default() -> Self {
        Self::new()
    }
}
