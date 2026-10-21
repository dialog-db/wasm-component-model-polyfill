// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The dynamic handle to a resource of any type, as Wasmtime's
//! `ResourceAny` (`crates/wasmtime/src/runtime/component/resources/any.rs`
//! at `v50.0.0`).

use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::internal::ResourceAnyInternal;
use crate::store::{StoreContext, StoreContextInternalExt};
use crate::types::{ResourceType, ValueType};

use super::handle_lookup_error::HandleLookupError;
use super::host_dynamic::ResourceDynamic;
use super::host_handle::HostHandle;
use super::host_handle_state::HostHandleState;
use super::host_static::Resource;

/// A handle to a resource of any type, a host's or a component's,
/// owned or borrowed.
///
/// The handle names an entry of the store's host table for its
/// resource type. A host gets one from a guest, through a function
/// that returns or takes an `own` or a `borrow`, or from a typed
/// handle through [`ResourceAny::try_from_resource`]. It is a plain
/// copyable record, and nothing checks its type until it crosses: a
/// lowering where the component declares another resource type fails
/// with the unregistered-resource-type ABI cause.
///
/// Every handle is released with [`ResourceAny::resource_drop`]. An
/// owned handle's release takes its entry out of the host table and
/// runs the resource's destructor. A borrow's entry goes when the
/// call that lent it ends, so its release only checks that the entry
/// is still there.
///
/// A copy kept past the release of its entry names nothing: the
/// table gives each entry that reuses an index a new generation, and
/// a handle of an old generation fails with "host-owned resource was
/// already de-allocated".
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceAny {
    /// The identity of the handle's resource type.
    ty: ResourceType,
    /// The index of the host table's entry.
    index: u32,
    /// The resource's `u32` rep, as the entry holds it. An entry of the
    /// same index and generation that holds another rep is one this
    /// handle was not made for.
    rep: u32,
    /// The generation of the host table's entry, shifted up past two
    /// flags: [`OWNED`], and [`DESTRUCTOR`]. A host table's generation
    /// stays below 2^30, so the three fit one word, and a `Val` that
    /// carries the handle stays 32 bytes, the size the copy budget
    /// charges for one.
    generation_flags: u32,
}

/// The flag of an owned handle, and not a borrow.
const OWNED: u32 = 1;

/// The flag of a handle whose release, when it is owned, runs the
/// resource's destructor. A handle made from a typed host handle runs
/// none, as in Wasmtime.
const DESTRUCTOR: u32 = 2;

/// The bits of [`ResourceAny::generation_flags`] below the generation.
const FLAG_BITS: u32 = 2;

/// The index of a synthetic borrow, which names no entry of the host
/// table. A host table reserves index 0, so no entry has it.
const SYNTHETIC: u32 = 0;

impl ResourceAny {
    fn of(
        ty: ResourceType,
        index: u32,
        rep: u32,
        generation: u32,
        owned: bool,
        destructor: bool,
    ) -> Self {
        debug_assert!(generation < 1 << (32 - FLAG_BITS));
        let flags = if owned { OWNED } else { 0 } | if destructor { DESTRUCTOR } else { 0 };
        Self {
            ty,
            index,
            rep,
            generation_flags: generation << FLAG_BITS | flags,
        }
    }
}

impl ResourceAny {
    /// The identity of the handle's resource type, which compares
    /// against [`ResourceType::host`] for a host resource.
    pub fn ty(&self) -> ResourceType {
        self.ty
    }

    /// Whether the handle is owned, and not a borrow.
    pub fn owned(&self) -> bool {
        self.generation_flags & OWNED != 0
    }

    /// Release the handle.
    ///
    /// An owned handle's entry leaves the host table, and the
    /// resource's destructor runs once: the host's registered closure
    /// for a host resource, or the defining component's destructor
    /// for one a component defines. A handle made with
    /// [`ResourceAny::try_from_resource`] runs none. A borrow's entry
    /// goes when the call that lent it ends, so its release checks the
    /// entry and leaves it. A borrow the host made itself, with
    /// [`Resource::new_borrow`] or [`ResourceDynamic::new_borrow`], has
    /// no entry, and its release does nothing, as in Wasmtime.
    ///
    /// The destructor runs as a task with one thread of its own, as
    /// it does when a guest drops the last owning handle, and may not
    /// block: a destructor that blocks fails the release with the
    /// cannot-block cause, on every target. A component's destructor
    /// is guest code, so the release runs a turn of the store's
    /// scheduler for it; a destructor that fails is a trap, and
    /// poisons the store. A poisoned store refuses the release of a
    /// resource a component defines with the cannot-enter cause, after
    /// the entry has left the host table, as in Wasmtime. A host
    /// resource's destructor is the host's own closure, so it runs
    /// outside a turn, and runs on a poisoned store too.
    ///
    /// Dropping the store instead runs no destructor: a handle the
    /// host never released is leaked, as in Wasmtime.
    ///
    /// # Errors
    ///
    /// A handle the table no longer holds, because it was released or
    /// lowered into a guest already, fails with the invalid-handle
    /// cause, and so does an owned handle lent out as a borrow until
    /// the call that borrowed it ends.
    pub fn resource_drop<D: 'static>(self, store: &mut StoreContext<'_, D>) -> Result<()> {
        if self.synthetic() {
            return Ok(());
        }
        store.internal().reachable()?;
        store.internal().resource_drop(self)
    }

    /// The handle of `resource`, as
    /// [`Resource::try_into_resource_any`] makes one.
    ///
    /// # Errors
    ///
    /// As [`Resource::try_into_resource_any`].
    pub fn try_from_resource<T: 'static, D: 'static>(
        resource: Resource<T>,
        store: &mut StoreContext<'_, D>,
    ) -> Result<Self> {
        resource.try_into_resource_any(store)
    }

    /// The typed handle of this one, as
    /// [`Resource::try_from_resource_any`] makes one.
    ///
    /// # Errors
    ///
    /// As [`Resource::try_from_resource_any`].
    pub fn try_into_resource<T: 'static>(
        self,
        store: &mut StoreContext<'_, impl Sized + 'static>,
    ) -> Result<Resource<T>> {
        Resource::try_from_resource_any(self, store)
    }

    /// The dynamic host handle of this one, as
    /// [`ResourceDynamic::try_from_resource_any`] makes one.
    ///
    /// # Errors
    ///
    /// As [`ResourceDynamic::try_from_resource_any`].
    pub fn try_into_resource_dynamic<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
    ) -> Result<ResourceDynamic> {
        ResourceDynamic::try_from_resource_any(self, store)
    }
}

impl ResourceAnyInternal for ResourceAny {
    fn new(ty: ResourceType, index: u32, rep: u32, generation: u32, owned: bool) -> Self {
        Self::of(ty, index, rep, generation, owned, true)
    }

    fn index(&self) -> u32 {
        self.index
    }

    fn rep(&self) -> u32 {
        self.rep
    }

    fn generation(&self) -> u32 {
        self.generation_flags >> FLAG_BITS
    }

    fn runs_destructor(&self) -> bool {
        self.generation_flags & DESTRUCTOR != 0
    }

    fn synthetic(&self) -> bool {
        self.index == SYNTHETIC
    }

    fn from_host<D: 'static>(
        store: &mut StoreContext<'_, D>,
        handle: &HostHandle,
        identity: ResourceType,
    ) -> Result<Self> {
        let rep = handle.rep();
        let refuse = |reason: String| refusal(identity, reason);
        let mut guard = store.internal().lock_tables()?;
        let (index, generation, owned) = match handle.state() {
            // A borrow the host made itself takes no entry, so it needs
            // no call in flight. Its rep goes to the guest when it is
            // lowered, as Wasmtime 50 defers it (`4ac4c8cbf`).
            HostHandleState::Borrow => (SYNTHETIC, 0, false),
            HostHandleState::NotInTable => {
                let (index, generation) = guard
                    .insert_host_own(identity, false, rep)
                    .map_err(|e| refuse(e.to_string()))?;
                (index, generation, true)
            }
            HostHandleState::Taken => return Err(refuse(CONSUMED.to_owned())),
            HostHandleState::Index { index, generation } => (index, generation, true),
        };
        Ok(Self::of(identity, index, rep, generation, owned, false))
    }

    fn into_host<D: 'static>(
        self,
        store: &mut StoreContext<'_, D>,
        identity: ResourceType,
    ) -> Result<HostHandle> {
        if self.ty != identity {
            return Err(resource_type_mismatch(self.ty));
        }
        if self.synthetic() {
            return Ok(HostHandle::borrow(self.rep));
        }
        let mut guard = store.internal().lock_tables()?;
        let table = guard.host_table(self.ty);
        let refuse = |error: HandleLookupError| refusal(self.ty, host_reason(error));
        if self.owned() {
            let rep = guard
                .remove_host_own(
                    table,
                    self.index,
                    self.generation(),
                    self.rep,
                    self.ty,
                    false,
                )
                .map_err(refuse)?;
            Ok(HostHandle::own(rep))
        } else {
            guard
                .lookup_host(
                    table,
                    self.index,
                    self.generation(),
                    self.rep,
                    self.ty,
                    false,
                )
                .map_err(refuse)?;
            Ok(HostHandle::borrow(self.rep))
        }
    }
}

/// Wasmtime's refusal of a host handle that a lowering as an `own`
/// moved into a guest already.
pub const CONSUMED: &str = "host resource already consumed";

/// The refusal of a conversion between handles of two resource types,
/// in Wasmtime's words.
pub fn resource_type_mismatch(ty: ResourceType) -> Error {
    refusal(ty, "resource type mismatch".to_owned())
}

/// The invalid-handle failure of a host operation on a handle of the
/// type `ty`, which names the handle as the operation's argument.
fn refusal(ty: ResourceType, reason: String) -> Error {
    Error::from(AbiError {
        position: AbiPosition::Argument(0),
        valtype: Some(ValueType::Own(ty)),
        cause: AbiCause::InvalidHandle { reason },
    })
}

/// What a refused lookup in the host table says, naming the host's
/// table where a bare index would read as a guest's mistake.
pub fn host_reason(error: HandleLookupError) -> String {
    match error {
        HandleLookupError::Unknown { index } => {
            format!("handle index {index} is not live in the host's resource table")
        }
        HandleLookupError::NotOwned { index } => {
            format!("handle index {index} is a borrow, which returns with its call")
        }
        other => other.to_string(),
    }
}
