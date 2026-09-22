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
use crate::internal::{HostResourceInternal, ResourceTypeIdInternal};
use crate::resource::ResourceTypeId;

/// One registered host resource inside a [`crate::LinkerInstance`].
///
/// Wraps the engine-issued identity and the destructor closure.
/// Both registration paths (`resource` and `resource_with`) produce
/// this single shape so the executor sees one carrier. Cloning shares
/// the identity, so one value registers the same resource type under
/// several interfaces.
///
/// The parts are readable and not writable. The identity is what the
/// engine minted for this registration, and the store keys both the
/// host's own table for the type and the type's destructor by it, so
/// a registration whose identity safe host code had chosen would file
/// its destructor under a name another registration answers to. An
/// identity is therefore minted and never assigned. The mint itself
/// is crate-internal — see [`ResourceTypeId`] — and [`Self::new`] is
/// the only way a caller outside reaches it, so neither half of the
/// carrier can be supplied:
///
/// ```compile_fail
/// # use std::sync::Arc;
/// # use wasm_component_model_polyfill::{HostResource, ResourceTypeId};
/// # fn forge(type_id: ResourceTypeId) -> HostResource<()> {
/// HostResource {
///     type_id,
///     destructor: Arc::new(|_: &mut (), _: u32| Ok(())),
/// }
/// # }
/// ```
///
/// Overwriting the identity of one a host built the ordinary way is
/// the same refusal:
///
/// ```compile_fail
/// # use wasm_component_model_polyfill::{HostResource, ResourceTypeId};
/// # fn steal(stolen: ResourceTypeId) {
/// let mut resource = HostResource::new(|_: &mut (), _: u32| Ok(()));
/// resource.type_id = stolen;
/// # }
/// ```
///
/// A host reads the identity through [`Self::type_id`]. The
/// destructor it registered is not readable at all: the closure is
/// the crate's to call when the guest drops a handle, and
/// [`DestructorBody`] is a type no name outside the crate resolves
/// to, so a `pub` field holding one would be callable by field
/// syntax and nameable by nothing. The carrier imports:
///
/// ```rust
/// use wasm_component_model_polyfill::HostResource;
/// let resource = HostResource::new(|_: &mut (), _: u32| Ok(()));
/// let _ = resource.type_id();
/// ```
///
/// The closure type it holds does not:
///
/// ```compile_fail
/// use wasm_component_model_polyfill::DestructorBody;
/// fn body() -> Box<DestructorBody<()>> {
///     Box::new(|_: &mut (), _: u32| Ok(()))
/// }
/// ```
pub struct HostResource<T> {
    /// The engine-issued identity for this registration. Every
    /// handle lookup carries it as the type check the entry has to
    /// match, so two registrations that share a label stay distinct;
    /// the store keys the host's own table for the type, and the
    /// type's destructor, by it.
    type_id: ResourceTypeId,
    /// The destructor the guest drop runs against. Takes the
    /// store's host-data slot and the resource's `u32` rep — the
    /// host-supplied 32-bit representation that identifies the
    /// resource's host-side state.
    destructor: Arc<DestructorBody<T>>,
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

    /// The engine-issued identity this registration was minted
    /// under, which is what
    /// [`LinkerInstance::resource_with`](crate::LinkerInstance::resource_with)
    /// returns when the registration is made.
    pub fn type_id(&self) -> ResourceTypeId {
        self.type_id
    }
}

impl<T> HostResourceInternal<T> for HostResource<T> {
    fn destructor(&self) -> &Arc<DestructorBody<T>> {
        &self.destructor
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
