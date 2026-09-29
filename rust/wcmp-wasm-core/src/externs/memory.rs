//! A linear memory.

use crate::checks;
use crate::error::{Error, Result};
use crate::internal::{StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut};
use crate::types::MemoryType;

handle! {
    /// A linear memory.
    ///
    /// An offset is a 64-bit number, so a memory addressed with 64-bit
    /// numbers uses the same methods. Each method checks its range against
    /// the size of the memory: a range outside the memory is
    /// [`Error::MemoryOutOfBounds`], never a panic and never an abort. The
    /// scalar loads and stores are little-endian, as WebAssembly is.
    ///
    /// The runtime layer never lends a shared memory as a slice, because
    /// another agent can write it at any time. Reads and writes of a shared
    /// memory are atomic.
    Memory
}

impl Memory {
    /// A memory of type `ty` in `store`.
    ///
    /// A memory addressed with 64-bit numbers needs
    /// [`memory64`](crate::Capability::Memory64), and a shared memory needs
    /// [`threads`](crate::Capability::Threads). Where the backend lacks the
    /// capability, this is [`Error::Unsupported`].
    pub fn new(mut store: impl AsContextMut, ty: MemoryType) -> Result<Self> {
        let mut store = store.as_context_mut();
        checks::memory_type(store.engine().capabilities(), &ty)?;
        store.backend_mut().memory_new(ty)
    }

    /// The type of the memory.
    pub fn ty(&self, store: impl AsContext) -> Result<MemoryType> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_ty(*self)
    }

    /// The size of the memory, in bytes.
    pub fn size(&self, store: impl AsContext) -> Result<u64> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_size(*self)
    }

    /// Grows the memory by `pages` pages of 64 KiB, and returns its old size
    /// in pages. A growth past the maximum of the memory is [`Error::Grow`].
    pub fn grow(&self, mut store: impl AsContextMut, pages: u64) -> Result<u64> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_grow(*self, pages)
    }

    /// Copies the bytes of the memory at `offset` into `buffer`.
    pub fn read(&self, store: impl AsContext, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_read(*self, offset, buffer)
    }

    /// Copies `bytes` into the memory at `offset`.
    pub fn write(&self, mut store: impl AsContextMut, offset: u64, bytes: &[u8]) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_write(*self, offset, bytes)
    }

    /// Reads the byte of the memory at `offset`.
    pub fn load_u8(&self, store: impl AsContext, offset: u64) -> Result<u8> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_load_u8(*self, offset)
    }

    /// Reads the `u16` of the memory at `offset`.
    pub fn load_u16(&self, store: impl AsContext, offset: u64) -> Result<u16> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_load_u16(*self, offset)
    }

    /// Reads the `u32` of the memory at `offset`.
    pub fn load_u32(&self, store: impl AsContext, offset: u64) -> Result<u32> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_load_u32(*self, offset)
    }

    /// Reads the `u64` of the memory at `offset`.
    pub fn load_u64(&self, store: impl AsContext, offset: u64) -> Result<u64> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.memory_load_u64(*self, offset)
    }

    /// Writes the byte `value` to the memory at `offset`.
    pub fn store_u8(&self, mut store: impl AsContextMut, offset: u64, value: u8) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_store_u8(*self, offset, value)
    }

    /// Writes the `u16` `value` to the memory at `offset`.
    pub fn store_u16(&self, mut store: impl AsContextMut, offset: u64, value: u16) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_store_u16(*self, offset, value)
    }

    /// Writes the `u32` `value` to the memory at `offset`.
    pub fn store_u32(&self, mut store: impl AsContextMut, offset: u64, value: u32) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_store_u32(*self, offset, value)
    }

    /// Writes the `u64` `value` to the memory at `offset`.
    pub fn store_u64(&self, mut store: impl AsContextMut, offset: u64, value: u64) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.memory_store_u64(*self, offset, value)
    }

    /// Lends the `len` bytes of the memory at `offset` to `f`, and returns
    /// what `f` returns.
    ///
    /// The store stays borrowed while `f` runs, so nothing can grow or
    /// write the memory meanwhile. Natively, over an unshared memory, the
    /// bytes are the memory's own, and the read copies nothing. A shared
    /// memory is always copied, with atomic reads, and `f` sees the copy.
    /// In the browser, the range is copied once into the host's own memory.
    pub fn with_bytes<R>(
        &self,
        store: impl AsContext,
        offset: u64,
        len: usize,
        f: impl FnOnce(&[u8]) -> R,
    ) -> Result<R> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        let mut f = Some(f);
        let mut result = None;
        backend.memory_with_bytes(*self, offset, len, &mut |bytes| {
            if let Some(f) = f.take() {
                result = Some(f(bytes));
            }
        })?;
        result.ok_or_else(|| Error::Backend {
            message: "the backend did not lend the bytes of the memory".to_string(),
        })
    }

    /// Copies `len` bytes from `source` at `source_offset` to `destination`
    /// at `destination_offset`, with no buffer on the host.
    ///
    /// Both memories belong to `store`. They can be one memory, and the
    /// ranges can overlap.
    pub fn copy(
        mut store: impl AsContextMut,
        source: &Memory,
        source_offset: u64,
        destination: &Memory,
        destination_offset: u64,
        len: u64,
    ) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *source)?;
        checks::same_store(backend, *destination)?;
        backend.memory_copy(
            *source,
            source_offset,
            *destination,
            destination_offset,
            len,
        )
    }
}
