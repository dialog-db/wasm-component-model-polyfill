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
use crate::runtime_layer::{Backend, StoreContextMut, Val as RuntimeVal};

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
        store: &mut StoreContextMut<'_, T, Backend>,
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
                    .read(&mut *store, offset, &mut buffer)
                    .map_err(AbiCause::SubstrateFailure)?;
                Ok(buffer)
            }
            Self::Lazy => Err(AbiCause::UnsupportedDataModel),
        }
    }

    /// Store `bytes` at `offset` through `options`.
    pub fn store<T: 'static>(
        &self,
        store: &mut StoreContextMut<'_, T, Backend>,
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
                    .write(&mut *store, offset, bytes)
                    .map_err(AbiCause::SubstrateFailure)
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
        store: &mut StoreContextMut<'_, T, Backend>,
        options: &BoundaryOptions,
        size: usize,
        alignment: usize,
    ) -> Result<usize, AbiCause> {
        match self {
            Self::Eager => {
                let realloc = options
                    .realloc()
                    .ok_or(AbiCause::ReallocUnavailable)?
                    .clone();
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
                    .map_err(AbiCause::ReallocFailed)?;
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
        store: &mut StoreContextMut<'_, T, Backend>,
        options: &BoundaryOptions,
    ) -> Option<usize> {
        match self {
            Self::Eager => {
                let memory = options.memory()?.clone();
                Some(memory.current_pages(&*store) as usize * 65536)
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
