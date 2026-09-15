use anyhow::Result;
use js_sys::{ArrayBuffer, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_runtime_layer::{
    backend::{AsContext, AsContextMut, WasmMemory},
    MemoryType,
};

use crate::{conversion::ToStoredJs, Engine, JsErrorMsg, StoreInner};

#[derive(Debug, Clone)]
/// WebAssembly memory
pub struct Memory {
    /// The id of the memory in the store
    pub id: usize,
}

#[derive(Debug)]
/// Holds the inner state of the memory
pub(crate) struct MemoryInner {
    /// The memory value
    pub value: WebAssembly::Memory,
    /// The memory type
    pub ty: MemoryType,
}

impl MemoryInner {
    /// Returns a `Uint8Array` view of the memory
    /// PATCH (wcmp): a view over `offset..offset + len` that is an error
    /// when the range leaves the buffer. `Uint8Array::new_with_byte_offset_and_length`
    /// throws a JS `RangeError` for such a range, and a JS exception
    /// thrown from inside a host call is not catchable by the caller
    /// (wasm-bindgen aborts on it), so the check happens here instead.
    pub(crate) fn checked_uint8array(&self, offset: usize, len: usize) -> Result<Uint8Array> {
        let buffer = self.value.buffer();
        let buffer = buffer.dyn_ref::<ArrayBuffer>().unwrap();
        let size = buffer.byte_length() as usize;
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= size)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "out of bounds memory access: {len} bytes at offset {offset} in a memory of {size} bytes"
                )
            })?;
        let _ = end;
        Ok(Uint8Array::new_with_byte_offset_and_length(
            buffer,
            offset as u32,
            len as u32,
        ))
    }
}

impl ToStoredJs for Memory {
    type Repr = WebAssembly::Memory;
    fn to_stored_js<T>(&self, store: &StoreInner<T>) -> Result<WebAssembly::Memory> {
        let memory = &store.memories[self.id];
        Ok(memory.value.clone())
    }
}

impl Memory {
    /// Construct a memory from an exported memory object
    pub(crate) fn from_exported_memory<T>(
        store: &mut StoreInner<T>,
        value: JsValue,
        ty: MemoryType,
    ) -> Option<Self> {
        let memory: &WebAssembly::Memory = value.dyn_ref()?;

        Some(store.insert_memory(MemoryInner {
            value: memory.clone(),
            ty,
        }))
    }
}

impl WasmMemory<Engine> for Memory {
    fn new(mut ctx: impl AsContextMut<Engine>, ty: MemoryType) -> Result<Self> {
        let desc = Object::new();
        Reflect::set(&desc, &"initial".into(), &ty.initial_pages().into()).unwrap();
        if let Some(maximum) = ty.maximum_pages() {
            Reflect::set(&desc, &"maximum".into(), &maximum.into()).unwrap();
        }

        let memory = WebAssembly::Memory::new(&desc).map_err(JsErrorMsg::from)?;

        let ctx: &mut StoreInner<_> = &mut *ctx.as_context_mut();

        Ok(ctx.insert_memory(MemoryInner { value: memory, ty }))
    }

    fn ty(&self, ctx: impl AsContext<Engine>) -> MemoryType {
        ctx.as_context().memories[self.id].ty
    }

    fn grow(&self, mut ctx: impl AsContextMut<Engine>, additional: u32) -> Result<u32> {
        let ctx: &mut StoreInner<_> = &mut *ctx.as_context_mut();

        let inner = &mut ctx.memories[self.id];
        Ok(inner.value.grow(additional))
    }

    fn current_pages(&self, _: impl AsContext<Engine>) -> u32 {
        todo!("Memory::current_pages is not yet supported in the js_wasm_runtime_layer backend")
    }

    fn read(&self, ctx: impl AsContext<Engine>, offset: usize, buffer: &mut [u8]) -> Result<()> {
        let ctx: &StoreInner<_> = &*ctx.as_context();
        let memory = &ctx.memories[self.id];

        // PATCH (wcmp): bounds-checked, see `checked_uint8array`.
        memory
            .checked_uint8array(offset, buffer.len())?
            .copy_to(buffer);

        Ok(())
    }

    fn write(
        &self,
        mut ctx: impl AsContextMut<Engine>,
        offset: usize,
        buffer: &[u8],
    ) -> Result<()> {
        let ctx: &mut StoreInner<_> = &mut *ctx.as_context_mut();

        let inner = &mut ctx.memories[self.id];
        // PATCH (wcmp): bounds-checked, see `checked_uint8array`.
        let dst = inner.checked_uint8array(offset, buffer.len())?;

        #[cfg(feature = "tracing")]
        tracing::debug!("writing {buffer:?} into guest");
        dst.copy_from(buffer);

        Ok(())
    }
}
