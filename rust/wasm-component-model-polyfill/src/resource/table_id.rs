//! The identity of one handle table.

use std::sync::atomic::{AtomicU64, Ordering};

/// The identity of one handle table. The polyfill keeps one table
/// per component instance, shared by every handle kind the instance
/// uses, plus one table per resource type for the host's own
/// handles; each table gets a fresh identity when it is created, so
/// no two tables in a store collide.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TableId(u64);

impl TableId {
    /// Mint a fresh, never-before-issued table identity.
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
