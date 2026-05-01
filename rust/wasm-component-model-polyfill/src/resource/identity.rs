//! Engine-issued opaque identity for a registered resource type.
//!
//! A `ResourceTypeId` is the unique key the polyfill uses to address
//! a host-registered resource type. The id is minted by the
//! [`Engine`] at registration time and threaded through the
//! polyfill's runtime state so handles minted against one
//! registration cannot collide with handles minted against another
//! — even when the two registrations share the same label.
//!
//! Identity is workspace-internal: the [`Engine`] hands one out
//! on demand, every other module compares ids by value, and
//! consumers never see the underlying integer.
//!
//! [`Engine`]: crate::Engine

use std::sync::atomic::{AtomicU64, Ordering};

/// An engine-issued unique identity for a registered resource type.
///
/// Two `ResourceTypeId` values compare equal only when they were
/// minted by the same call to [`ResourceTypeId::fresh`]. The wrapped
/// integer is opaque and not exposed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceTypeId(u64);

impl ResourceTypeId {
    /// Mint a globally unique resource-type identity.
    ///
    /// The counter is process-wide; collisions across engines are
    /// not possible while the process is alive.
    pub fn fresh() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}
