// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A linear memory of a core instance of a component.

use crate::error::{Error, InstantiationError, Result};
use crate::internal::{ErrorInternal, MemoryParts};
use crate::runtime_layer::{Memory as RuntimeMemory, substrate_failure};
use crate::store::{Store, StoreId, StoreInternalExt};

/// The size of a page of a memory, in bytes. The runtime layer gives
/// every memory pages of 64 KiB.
const PAGE_SIZE: u64 = 65_536;

/// A linear memory that a core instance of a component defines.
///
/// A `Memory` comes from [`Instance::core_memories`]. It is the
/// polyfill's analogue to `wasmtime::Memory`, and it gives the host
/// the current size of the memory: [`Self::size`] in pages and
/// [`Self::data_size`] in bytes, as Wasmtime's methods of those names
/// do. A host reads the size to learn how much memory a guest holds,
/// for example to tell whether a guest that trapped had grown its
/// memory to the maximum the host set. The host does not read or
/// write the bytes: the component model moves values across the
/// boundary through lifted functions.
///
/// The memory lives in the [`Store`] its instance was created in. A
/// read through another store fails with
/// [`InstantiationError::WrongStore`]. A read runs no guest code, so
/// it works in a store that a trap poisoned, and it gives the size the
/// memory had when the trap stopped the guest.
///
/// Wasmtime's methods take any context of the store and panic on
/// another store. These take the [`Store`] and return a [`Result`].
///
/// [`Instance::core_memories`]: crate::Instance::core_memories
#[derive(Clone, Copy)]
pub struct Memory {
    /// The runtime-layer memory.
    inner: RuntimeMemory,
    /// The identity of the store the memory lives in.
    store_id: StoreId,
}

impl From<MemoryParts> for Memory {
    fn from(parts: MemoryParts) -> Self {
        Self {
            inner: parts.inner,
            store_id: parts.store_id,
        }
    }
}

impl Memory {
    /// The current size of the memory, in pages of 64 KiB, as
    /// `wasmtime::Memory::size` gives it.
    ///
    /// The size grows when the guest runs `memory.grow`, and never
    /// shrinks. A memory with a maximum never grows past it, so a
    /// size equal to the maximum is a memory that cannot grow.
    pub fn size<T: 'static>(&self, store: &Store<T>) -> Result<u64> {
        Ok(self.bytes(store)? / PAGE_SIZE)
    }

    /// The current size of the memory, in bytes, as
    /// `wasmtime::Memory::data_size` gives it. The size is always a
    /// whole number of pages of 64 KiB.
    ///
    /// A memory larger than the host can address fails with
    /// [`Error::Unsupported`]. Only a 64-bit memory past 4 GiB in the
    /// browser is that large.
    pub fn data_size<T: 'static>(&self, store: &Store<T>) -> Result<usize> {
        let bytes = self.bytes(store)?;
        usize::try_from(bytes).map_err(|_| {
            Error::unsupported(format!(
                "the size of a memory of {bytes} bytes, more than the host can address"
            ))
        })
    }

    /// The current size of the memory in bytes, read through `store`.
    fn bytes<T: 'static>(&self, store: &Store<T>) -> Result<u64> {
        if store.internal_ref().id() != self.store_id {
            return Err(Error::from(InstantiationError::WrongStore));
        }
        store.internal_ref().reachable()?;
        self.inner
            .size(store.internal_ref().inner())
            .map_err(substrate_failure)
    }
}

impl core::fmt::Debug for Memory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Memory").finish_non_exhaustive()
    }
}
