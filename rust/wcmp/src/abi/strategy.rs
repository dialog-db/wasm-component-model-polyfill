// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The ABI strategy a boundary crossing runs under.
//!
//! The canonical ABI has one strategy per data model. The eager
//! strategy is the linear-memory one: it stores a value into the
//! memory the caller supplied or `cabi_realloc` returned, and it
//! loads a value from a pointer. A lazy strategy, which the
//! garbage-collected data model needs, sits beside it behind the
//! same [`BoundaryContext`] and performs no linear-memory access.
//!
//! A [`BoundaryContext`] selects its strategy from its options when
//! it is built, so a crossing under a second strategy needs no
//! change at the call site that builds the context.
//!
//! [`BoundaryContext`]: crate::abi::context::BoundaryContext

use crate::abi::options::BoundaryOptions;
use crate::error::AbiCause;
use crate::executor::ir::DataModel;
use crate::runtime_layer::{Memory, StoreContextMut, Val as RuntimeVal, into_anyhow};

/// Which canonical-ABI strategy a crossing performs its accesses
/// under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiStrategy {
    /// The linear-memory strategy: a value is stored into the memory
    /// the caller supplied or `cabi_realloc` returned, and loaded
    /// from a pointer. The whole of the value is written before the
    /// crossing returns, which is what makes it eager.
    Eager,
    /// The strategy of a data model whose values do not live in
    /// linear memory. The polyfill implements no such strategy, so
    /// every access under it reports
    /// [`AbiCause::UnsupportedDataModel`].
    Lazy,
}

impl AbiStrategy {
    /// The strategy `options` select. The data model decides: linear
    /// memory takes the eager strategy, anything else the lazy one.
    pub fn select(options: &BoundaryOptions) -> Self {
        match options.data_model() {
            DataModel::LinearMemory => Self::Eager,
            DataModel::Gc => Self::Lazy,
        }
    }

    /// Load `length` bytes at `offset` through `options`.
    pub fn load<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
        offset: usize,
        length: usize,
    ) -> Result<Vec<u8>, AbiCause> {
        match self {
            Self::Eager => {
                let memory = options
                    .memory()
                    .ok_or(AbiCause::OutOfBoundsMemory { offset, length })?;
                let mut buffer = vec![0u8; length];
                #[cfg(test)]
                count_access(|(reads, writes)| (reads + 1, writes));
                memory
                    .read(&mut *store, offset as u64, &mut buffer)
                    .map_err(|error| AbiCause::SubstrateFailure(into_anyhow(error)))?;
                Ok(buffer)
            }
            Self::Lazy => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// Lend the `length` bytes at `offset` through `options` to `f`,
    /// and return what `f` returns. The runtime layer lends the
    /// memory's own bytes where it can, so natively the read copies
    /// nothing, and in the browser it copies the range once.
    pub fn with_bytes<T: 'static, R>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
        offset: usize,
        length: usize,
        f: impl FnOnce(&[u8]) -> R,
    ) -> Result<R, AbiCause> {
        match self {
            Self::Eager => {
                let memory = options
                    .memory()
                    .ok_or(AbiCause::OutOfBoundsMemory { offset, length })?;
                #[cfg(test)]
                count_access(|(reads, writes)| (reads + 1, writes));
                memory
                    .with_bytes(&mut *store, offset as u64, length, f)
                    .map_err(|error| AbiCause::SubstrateFailure(into_anyhow(error)))
            }
            Self::Lazy => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// Copy the `length` bytes at `source_offset` of the side
    /// `source` names to `offset` through `options`, from one guest
    /// memory to the other with no buffer on the host. Both sides
    /// must address linear memory.
    pub fn copy<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
        source: (&AbiStrategy, &BoundaryOptions),
        source_offset: usize,
        offset: usize,
        length: usize,
    ) -> Result<(), AbiCause> {
        match (source.0, self) {
            (Self::Eager, Self::Eager) => {
                let from = source.1.memory().ok_or(AbiCause::OutOfBoundsMemory {
                    offset: source_offset,
                    length,
                })?;
                let to = options
                    .memory()
                    .ok_or(AbiCause::OutOfBoundsMemory { offset, length })?;
                #[cfg(test)]
                MEMORY_COPIES.with(|copies| copies.set(copies.get() + 1));
                Memory::copy(
                    &mut *store,
                    from,
                    source_offset as u64,
                    to,
                    offset as u64,
                    length as u64,
                )
                .map_err(|error| AbiCause::SubstrateFailure(into_anyhow(error)))
            }
            _ => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// Store `bytes` at `offset` through `options`.
    pub fn store<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), AbiCause> {
        match self {
            Self::Eager => {
                let memory = options.memory().ok_or(AbiCause::OutOfBoundsMemory {
                    offset,
                    length: bytes.len(),
                })?;
                #[cfg(test)]
                count_access(|(reads, writes)| (reads, writes + 1));
                memory
                    .write(&mut *store, offset as u64, bytes)
                    .map_err(|error| AbiCause::SubstrateFailure(into_anyhow(error)))
            }
            Self::Lazy => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// Ask the guest for `size` bytes at `alignment` and return the
    /// pointer it gave back. The eager strategy calls the guest's
    /// `cabi_realloc` and checks the pointer it returns the way
    /// Wasmtime does: aligned as asked, and inside the memory.
    pub fn allocate<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
        size: usize,
        alignment: usize,
    ) -> Result<usize, AbiCause> {
        match self {
            Self::Eager => {
                let realloc = *options.realloc().ok_or(AbiCause::ReallocUnavailable)?;
                let args = [
                    RuntimeVal::I32(0), // old_ptr
                    RuntimeVal::I32(0), // old_size
                    RuntimeVal::I32(alignment as i32),
                    RuntimeVal::I32(size as i32),
                ];
                let mut results = [RuntimeVal::I32(0)];
                // The call carries whatever it failed with rather
                // than a rendering of it, so a host can still read
                // the error a `cabi_realloc` failed with.
                realloc
                    .call(&mut *store, &args, &mut results)
                    .map_err(|error| AbiCause::ReallocFailed(into_anyhow(error)))?;
                let RuntimeVal::I32(ptr) = results[0] else {
                    return Err(AbiCause::ReallocFailed(anyhow::anyhow!(
                        "cabi_realloc returned a non-i32 pointer"
                    )));
                };
                let ptr = ptr as u32 as usize;
                let rejected = |reason: &str| AbiCause::ReallocReturn {
                    reason: reason.to_owned(),
                };
                if alignment > 1 && !ptr.is_multiple_of(alignment) {
                    return Err(rejected("result not aligned"));
                }
                if let Some(size_of_memory) = self.size(store, options) {
                    let end = ptr
                        .checked_add(size)
                        .ok_or_else(|| rejected("beyond end of memory"))?;
                    if end > size_of_memory {
                        return Err(rejected("beyond end of memory"));
                    }
                }
                Ok(ptr)
            }
            Self::Lazy => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// How many bytes the guest holds for `options`, when the
    /// strategy addresses a store of a bounded size.
    pub fn size<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T>,
        options: &BoundaryOptions,
    ) -> Option<usize> {
        match self {
            Self::Eager => {
                let memory = *options.memory()?;
                memory
                    .size(&*store)
                    .ok()
                    .and_then(|size| usize::try_from(size).ok())
            }
            Self::Lazy => None,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many runtime-layer reads and writes of a guest memory the
    /// eager strategy has made on this thread, which a test reads
    /// around one crossing to count that crossing's accesses. A
    /// boundary context counts its own accesses too, but a built-in
    /// builds and drops its context inside one call of it, so a test
    /// that drives the built-in through a component cannot read that
    /// count; this one outlives the call and keeps reads apart from
    /// writes.
    static MEMORY_ACCESSES: std::cell::Cell<(usize, usize)> =
        const { std::cell::Cell::new((0, 0)) };

    /// How many copies from one guest memory to another, with no host
    /// buffer between them, the eager strategy has made on this thread,
    /// which are neither reads nor writes of the host's.
    static MEMORY_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count one runtime-layer access of a guest memory.
#[cfg(test)]
fn count_access(step: impl FnOnce((usize, usize)) -> (usize, usize)) {
    MEMORY_ACCESSES.with(|accesses| accesses.set(step(accesses.get())));
}

/// How many runtime-layer reads and writes of a guest memory the
/// eager strategy has made on this thread so far, reads first.
#[cfg(test)]
pub fn memory_accesses() -> (usize, usize) {
    MEMORY_ACCESSES.with(std::cell::Cell::get)
}

/// How many guest-to-guest copies the eager strategy has made on this
/// thread so far.
#[cfg(test)]
pub fn memory_copies() -> usize {
    MEMORY_COPIES.with(std::cell::Cell::get)
}
