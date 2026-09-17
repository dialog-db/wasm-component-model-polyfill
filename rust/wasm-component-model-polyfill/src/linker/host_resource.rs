//! Per-resource-type host registration payload stored on a
//! [`LinkerInstance`].
//!
//! `HostResource<T>` is the runtime carrier for what a developer
//! registers via [`LinkerInstance::resource`]: the engine-issued
//! identity that names this registration alongside the synchronous
//! destructor closure that runs when the guest drops a handle to the
//! resource.
//!
//! [`LinkerInstance`]: super::LinkerInstance
//! [`LinkerInstance::resource`]: super::LinkerInstance::resource

use std::sync::Arc;

use crate::error::Result;
use crate::resource::ResourceTypeId;

/// One registered host resource inside a [`crate::LinkerInstance`].
///
/// Wraps the engine-issued identity and the destructor closure.
/// Both registration paths (`resource` and `resource_with`) produce
/// this single shape so the executor sees one carrier. Cloning shares
/// the identity, so one value registers the same resource type under
/// several interfaces.
pub struct HostResource<T> {
    /// The engine-issued identity for this registration. Every
    /// handle lookup carries it as the type check the entry has to
    /// match, so two registrations that share a label stay distinct;
    /// the store keys the host's own table for the type, and the
    /// type's destructor, by it.
    pub type_id: ResourceTypeId,
    /// The destructor the guest drop runs against. Takes the
    /// store's host-data slot and the resource's `u32` rep — the
    /// host-supplied 32-bit representation that identifies the
    /// resource's host-side state.
    pub destructor: Arc<DestructorBody<T>>,
}

/// The closure type a [`HostResource`] holds.
///
/// The closure takes `&mut T` (the store's host-data slot) and the
/// resource's rep, and runs synchronously when the guest drops the
/// last handle. Returning `Err(_)` surfaces the error through the
/// drop trampoline as a structured failure.
pub type DestructorBody<T> = dyn Fn(&mut T, u32) -> Result<()> + Send + Sync + 'static;

impl<T> HostResource<T> {
    /// Construct a registration from a destructor closure. A fresh
    /// [`ResourceTypeId`] is minted; the caller never sees the
    /// underlying integer.
    pub fn new(destructor: impl Fn(&mut T, u32) -> Result<()> + Send + Sync + 'static) -> Self {
        Self {
            type_id: ResourceTypeId::fresh(),
            destructor: Arc::new(destructor),
        }
    }
}

impl<T> Clone for HostResource<T> {
    /// A clone shares the identity and the destructor, so one value
    /// registers the same resource type under several interfaces.
    fn clone(&self) -> Self {
        Self {
            type_id: self.type_id,
            destructor: self.destructor.clone(),
        }
    }
}
