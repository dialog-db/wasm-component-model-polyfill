//! Per-call canonical-ABI context.
//!
//! Lifts and lowers thread one of these through the per-valtype
//! recursion so the leaf paths can read or write guest memory and
//! invoke `cabi_realloc` / `post-return` without each call site
//! re-resolving them.
//!
//! The context borrows the runtime-layer [`Memory`] and the
//! optional `cabi_realloc` and `post-return` runtime-layer
//! [`Func`]s; each holds onto the
//! [`crate::executor::ir::StringEncoding`] the canon options
//! declared so the string lift/lower picks the right path.
//!
//! Construction is workspace-internal — the context is always built
//! immediately before a call drives the canonical ABI.

use std::sync::{Arc, Mutex};

use crate::backend::Backend;
use crate::error::{AbiCause, AbiError, AbiPosition, Error, Result};
use crate::executor::ir::StringEncoding;
use crate::resource::{HandleTables, ResourceTableRuntime};
use crate::types::ValueType;
use wasm_runtime_layer::{Func as RuntimeFunc, Memory, StoreContextMut, Val as RuntimeVal};

/// Read-side context for a canonical-ABI lift.
///
/// The store context is held as a mutable
/// [`StoreContextMut`] rather than a reference to the polyfill's
/// `Store` so the same context type works for both export-call
/// (`Func::call`) and host-trampoline call sites — only the latter
/// receives a `StoreContextMut`.
pub struct LiftContext<'a, T: 'static> {
    /// The runtime-layer store context the lift is reading from.
    pub store: StoreContextMut<'a, T, Backend>,
    /// The guest memory the lifted value sits in. `None` is rejected
    /// at the first compound-valtype access; primitives that fit in
    /// flat slots do not need it.
    pub memory: Option<Memory>,
    /// The string encoding the lift uses for `string`-typed values.
    pub string_encoding: StringEncoding,
    /// The per-store handle tables. Required when lifting `own<T>`
    /// or `borrow<T>` valtypes; `None` is rejected at first contact.
    pub tables: Option<Arc<Mutex<HandleTables>>>,
    /// Every resource table of the component instance, by table
    /// index. A handle's declared type names the index; this maps it
    /// to the table the instance keeps and the resource it holds.
    pub resource_tables: Vec<Option<ResourceTableRuntime>>,
}

impl<'a, T: 'static> LiftContext<'a, T> {
    /// Construct a lift context.
    pub fn new(
        store: StoreContextMut<'a, T, Backend>,
        memory: Option<Memory>,
        string_encoding: StringEncoding,
        tables: Option<Arc<Mutex<HandleTables>>>,
        resource_tables: Vec<Option<ResourceTableRuntime>>,
    ) -> Self {
        Self {
            store,
            memory,
            string_encoding,
            tables,
            resource_tables,
        }
    }

    /// The size of the guest memory in bytes, when the context has
    /// one.
    pub fn memory_size(&mut self) -> Option<usize> {
        let memory = self.memory.clone()?;
        Some(memory.current_pages(&self.store) as usize * 65536)
    }

    /// Whether `length` bytes at `offset` lie inside the guest memory.
    /// `true` when the context has no memory, so the read reports
    /// the absence itself.
    pub fn in_bounds(&mut self, offset: usize, length: usize) -> bool {
        match self.memory_size() {
            Some(size) => offset.checked_add(length).is_some_and(|end| end <= size),
            None => true,
        }
    }

    /// Read `length` bytes starting at `offset` from the guest's
    /// linear memory. Surfaces a structured [`AbiError`] when the
    /// memory is absent or the access is out of bounds.
    pub fn read_bytes(
        &mut self,
        offset: usize,
        length: usize,
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<Vec<u8>> {
        let Some(memory) = &self.memory else {
            return Err(Error::from(AbiError {
                position,
                valtype: valtype.clone(),
                cause: AbiCause::OutOfBoundsMemory { offset, length },
            }));
        };
        let mut buffer = vec![0u8; length];
        memory
            .read(&mut self.store, offset, &mut buffer)
            .map_err(|cause| {
                Error::from(AbiError {
                    position,
                    valtype: valtype.clone(),
                    cause: AbiCause::SubstrateFailure(cause),
                })
            })?;
        Ok(buffer)
    }
}

/// Write-side context for a canonical-ABI lower.
pub struct LowerContext<'a, T: 'static> {
    /// The runtime-layer store context the lower is writing through.
    pub store: StoreContextMut<'a, T, Backend>,
    /// The guest memory the lowered value will sit in. Required for
    /// every heap-allocating valtype.
    pub memory: Option<Memory>,
    /// The guest's `cabi_realloc` function. Required by every
    /// heap-allocating lower.
    pub realloc: Option<RuntimeFunc>,
    /// The string encoding the lower uses for `string`-typed values.
    pub string_encoding: StringEncoding,
    /// The per-store handle tables. Required when lowering `own<T>`
    /// or `borrow<T>` valtypes; `None` is rejected at first contact.
    pub tables: Option<Arc<Mutex<HandleTables>>>,
    /// Every resource table of the component instance, by table
    /// index. A handle's declared type names the index; this maps it
    /// to the table the instance keeps and the resource it holds.
    pub resource_tables: Vec<Option<ResourceTableRuntime>>,
}

impl<'a, T: 'static> LowerContext<'a, T> {
    /// Construct a lower context.
    pub fn new(
        store: StoreContextMut<'a, T, Backend>,
        memory: Option<Memory>,
        realloc: Option<RuntimeFunc>,
        string_encoding: StringEncoding,
        tables: Option<Arc<Mutex<HandleTables>>>,
        resource_tables: Vec<Option<ResourceTableRuntime>>,
    ) -> Self {
        Self {
            store,
            memory,
            realloc,
            string_encoding,
            tables,
            resource_tables,
        }
    }

    /// Write `bytes` at `offset` in the guest's linear memory.
    pub fn write_bytes(
        &mut self,
        offset: usize,
        bytes: &[u8],
        position: AbiPosition,
        valtype: &ValueType,
    ) -> Result<()> {
        let Some(memory) = &self.memory else {
            return Err(Error::from(AbiError {
                position,
                valtype: valtype.clone(),
                cause: AbiCause::OutOfBoundsMemory {
                    offset,
                    length: bytes.len(),
                },
            }));
        };
        memory
            .write(&mut self.store, offset, bytes)
            .map_err(|cause| {
                Error::from(AbiError {
                    position,
                    valtype: valtype.clone(),
                    cause: AbiCause::SubstrateFailure(cause),
                })
            })
    }

    /// Allocate `size` bytes of guest memory at an explicit
    /// `alignment` by invoking the guest's `cabi_realloc`. `valtype`
    /// and `position` only label the error when the allocation
    /// fails. Returns the new pointer (a guest-memory offset).
    pub fn allocate_aligned(
        &mut self,
        size: usize,
        alignment: usize,
        valtype: &ValueType,
        position: AbiPosition,
    ) -> Result<usize> {
        let Some(realloc) = self.realloc.clone() else {
            return Err(Error::from(AbiError {
                position,
                valtype: valtype.clone(),
                cause: AbiCause::ReallocUnavailable,
            }));
        };
        let args = [
            RuntimeVal::I32(0), // old_ptr
            RuntimeVal::I32(0), // old_size
            RuntimeVal::I32(alignment as i32),
            RuntimeVal::I32(size as i32),
        ];
        let mut results = [RuntimeVal::I32(0)];
        realloc
            .call(&mut self.store, &args, &mut results)
            .map_err(|cause| {
                Error::from(AbiError {
                    position,
                    valtype: valtype.clone(),
                    cause: AbiCause::ReallocFailed(cause),
                })
            })?;
        let ptr = match results[0] {
            RuntimeVal::I32(ptr) => ptr as u32 as usize,
            _ => {
                return Err(Error::from(AbiError {
                    position,
                    valtype: valtype.clone(),
                    cause: AbiCause::ReallocFailed(anyhow::anyhow!(
                        "cabi_realloc returned a non-i32 pointer"
                    )),
                }));
            }
        };
        // The pointer must be usable: aligned as asked and inside
        // memory, as Wasmtime checks after every realloc.
        let realloc_return = |reason: &str| {
            Error::from(AbiError {
                position,
                valtype: valtype.clone(),
                cause: AbiCause::ReallocReturn {
                    reason: reason.to_owned(),
                },
            })
        };
        if alignment > 1 && !ptr.is_multiple_of(alignment) {
            return Err(realloc_return("result not aligned"));
        }
        if let Some(size_of_memory) = self.memory_size() {
            let end = ptr
                .checked_add(size)
                .ok_or_else(|| realloc_return("beyond end of memory"))?;
            if end > size_of_memory {
                return Err(realloc_return("beyond end of memory"));
            }
        }
        Ok(ptr)
    }

    /// The size of the guest memory in bytes, when the context has
    /// one.
    pub fn memory_size(&mut self) -> Option<usize> {
        let memory = self.memory.clone()?;
        Some(memory.current_pages(&self.store) as usize * 65536)
    }
}
