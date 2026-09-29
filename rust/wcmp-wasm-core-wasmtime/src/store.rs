//! A store of the Wasmtime backend, as the runtime layer reaches it.

use core::any::Any;
use core::ops::Range;
use core::sync::atomic::{AtomicU8, Ordering};
use core::task::Poll;
use std::cell::UnsafeCell;

use wasmtime::{AsContextMut, RootScope};
use wcmp_wasm_core::backend::{BackendModule, BackendStore, BoxFuture, HostFunc, StoreData};
use wcmp_wasm_core::{
    AnyRef, Capability, Error, Extern, ExternRef, Func, FuncType, Global, GlobalType, I31,
    Instance, Memory, MemoryType, Result, Table, TableType, Tag, TagType, Val,
};

use crate::context::Context;
use crate::convert;
use crate::errors;
use crate::host_error::HostError;
use crate::memory_object::MemoryObject;
use crate::module::WasmtimeModule;
use crate::values;

/// A store of the Wasmtime backend, over a Wasmtime context `C`.
///
/// The context is the Wasmtime store itself, which the runtime layer's
/// `Store` owns, or the `Caller` a host function receives while a guest
/// runs in the store. One implementation serves both, so a host function
/// reaches the store exactly as the host does, and can call back into the
/// guest at any depth.
pub struct WasmtimeStore<C> {
    inner: C,
}

impl<C: Context> WasmtimeStore<C> {
    /// The store over the Wasmtime context `inner`.
    pub fn new(inner: C) -> Self {
        Self { inner }
    }

    /// Instantiates `module` with `imports`, synchronously, as Wasmtime
    /// does.
    fn instantiate_now(
        &mut self,
        module: &dyn BackendModule,
        imports: &[Extern],
    ) -> Result<Instance> {
        let module = module
            .as_any()
            .downcast_ref::<WasmtimeModule>()
            .ok_or(Error::WrongEngine)?;
        if !wasmtime::Engine::same(module.module().engine(), self.inner.as_context().engine()) {
            return Err(Error::WrongEngine);
        }
        let imports_of_module = module.imports();
        let imports = imports
            .iter()
            .map(|import| self.inner.state().to_extern(import))
            .collect::<Result<Vec<_>>>()?;
        let mut scope = RootScope::new(&mut self.inner);
        match wasmtime::Instance::new(&mut scope, module.module(), &imports) {
            Ok(instance) => Ok(scope.as_context_mut().data_mut().add_instance(instance)),
            Err(error) => Err(errors::instantiation(&mut scope, imports_of_module, error)),
        }
    }

    /// The memory that `memory` names.
    fn memory_object(&self, memory: Memory) -> Result<MemoryObject> {
        self.inner.state().memory(memory).cloned()
    }

    /// The range of the `len` bytes at `offset` of `memory`, where the
    /// memory holds them all.
    fn range(&self, memory: &MemoryObject, offset: u64, len: u64) -> Result<Range<usize>> {
        let size = self.size_of(memory);
        let out_of_bounds = || Error::MemoryOutOfBounds { offset, len, size };
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= size)
            .ok_or_else(out_of_bounds)?;
        // The memory holds `end` bytes in the host's address space, so both
        // numbers fit in a `usize`.
        let start = usize::try_from(offset).map_err(|_| out_of_bounds())?;
        let end = usize::try_from(end).map_err(|_| out_of_bounds())?;
        Ok(start..end)
    }

    /// The size of `memory`, in bytes.
    fn size_of(&self, memory: &MemoryObject) -> u64 {
        match memory {
            MemoryObject::Unshared(memory) => memory.data_size(&self.inner) as u64,
            MemoryObject::Shared(memory) => memory.data_size() as u64,
        }
    }
}

impl<C: Context> BackendStore for WasmtimeStore<C> {
    fn data(&self) -> &StoreData {
        self.inner.state().data()
    }

    fn data_mut(&mut self) -> &mut StoreData {
        self.inner.state_mut().data_mut()
    }

    fn instantiate<'a>(
        &'a mut self,
        module: &'a dyn BackendModule,
        imports: &'a [Extern],
    ) -> BoxFuture<'a, Result<Instance>> {
        // Wasmtime instantiates synchronously, so the future is ready on its
        // first poll.
        Box::pin(core::future::ready(self.instantiate_now(module, imports)))
    }

    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
        let instance = *self.inner.state().instance(instance)?;
        Ok(instance
            .get_export(&mut self.inner, name)
            .map(|export| self.inner.state_mut().add_extern(export)))
    }

    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
        if func.is_suspending() {
            return Err(Error::Unsupported(Capability::HostSuspension));
        }
        let types = self.inner.state().types().clone();
        let wasm_ty = convert::to_func_type(self.inner.as_context().engine(), &types, &ty)?;
        values::refuse_continuations(wasm_ty.params().chain(wasm_ty.results()))?;
        let result_types = wasm_ty.results().collect::<Vec<_>>();
        let slots = ty
            .results()
            .iter()
            .map(|ty| Val::default_for_ty(ty).unwrap_or(Val::I32(0)))
            .collect::<Vec<_>>();
        let host =
            wasmtime::Func::try_new(&mut self.inner, wasm_ty, move |caller, params, results| {
                call_host(&func, &slots, &result_types, caller, params, results)
            })
            .map_err(errors::backend)?;
        Ok(self.inner.state_mut().add_func(host))
    }

    fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
        let func = self.inner.state().func(func)?;
        let types = self.inner.state().types();
        Ok(Some(convert::func_type(types, &func.ty(&self.inner))))
    }

    fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()> {
        let func = *self.inner.state().func(func)?;
        let ty = func.ty(&self.inner);
        if params.len() != ty.params().len() || results.len() != ty.results().len() {
            return Err(Error::TypeMismatch {
                message: format!(
                    "the function takes {} arguments and gives {} results, and the call gave {} \
                     arguments and {} result slots",
                    ty.params().len(),
                    ty.results().len(),
                    params.len(),
                    results.len(),
                ),
            });
        }
        values::refuse_continuations(ty.params().chain(ty.results()))?;
        let mut scope = RootScope::new(&mut self.inner);
        let arguments = params
            .iter()
            .zip(ty.params())
            .map(|(value, ty)| values::to_wasmtime(&mut scope, value, &ty))
            .collect::<Result<Vec<_>>>()?;
        let mut outputs = vec![wasmtime::Val::I32(0); results.len()];
        func.call(&mut scope, &arguments, &mut outputs)
            .map_err(|error| errors::trap(&mut scope, error))?;
        for (slot, output) in results.iter_mut().zip(&outputs) {
            *slot = values::from_wasmtime(&mut scope, output)?;
        }
        Ok(())
    }

    // `func_call_resumable` and `resume_call` keep the contract's bodies,
    // which return `Unsupported(host_suspension)`: see the crate's
    // documentation of its capabilities.

    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
        let wasm_ty = convert::to_memory_type(&ty)?;
        let memory = if ty.is_shared() {
            let engine = self.inner.as_context().engine().clone();
            MemoryObject::Shared(
                wasmtime::SharedMemory::new(&engine, wasm_ty).map_err(errors::backend)?,
            )
        } else {
            MemoryObject::Unshared(
                wasmtime::Memory::new(&mut self.inner, wasm_ty).map_err(errors::backend)?,
            )
        };
        Ok(self.inner.state_mut().add_memory(memory))
    }

    fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
        Ok(match self.inner.state().memory(memory)? {
            MemoryObject::Unshared(memory) => convert::memory_type(&memory.ty(&self.inner)),
            MemoryObject::Shared(memory) => convert::memory_type(&memory.ty()),
        })
    }

    fn memory_size(&self, memory: Memory) -> Result<u64> {
        let memory = self.inner.state().memory(memory)?;
        Ok(self.size_of(memory))
    }

    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
        let grown = match self.memory_object(memory)? {
            MemoryObject::Unshared(memory) => memory.grow(&mut self.inner, pages),
            MemoryObject::Shared(memory) => memory.grow(pages),
        };
        grown.map_err(|_| Error::Grow { delta: pages })
    }

    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let memory = self.inner.state().memory(memory)?;
        let range = self.range(memory, offset, buffer.len() as u64)?;
        match memory {
            MemoryObject::Unshared(memory) => {
                buffer.copy_from_slice(&memory.data(&self.inner)[range]);
            }
            MemoryObject::Shared(memory) => {
                for (byte, cell) in buffer.iter_mut().zip(&memory.data()[range]) {
                    *byte = atomic(cell).load(Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()> {
        let memory = self.memory_object(memory)?;
        let range = self.range(&memory, offset, bytes.len() as u64)?;
        match memory {
            MemoryObject::Unshared(memory) => {
                memory.data_mut(&mut self.inner)[range].copy_from_slice(bytes);
            }
            MemoryObject::Shared(memory) => {
                for (byte, cell) in bytes.iter().zip(&memory.data()[range]) {
                    atomic(cell).store(*byte, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        let memory = self.inner.state().memory(memory)?;
        let range = self.range(memory, offset, len as u64)?;
        match memory {
            MemoryObject::Unshared(memory) => f(&memory.data(&self.inner)[range]),
            // Another agent can write a shared memory at any time, so the
            // store never lends it: it lends a copy, read atomically.
            MemoryObject::Shared(memory) => {
                let copy = memory.data()[range]
                    .iter()
                    .map(|cell| atomic(cell).load(Ordering::SeqCst))
                    .collect::<Vec<_>>();
                f(&copy);
            }
        }
        Ok(())
    }

    fn memory_copy(
        &mut self,
        source: Memory,
        source_offset: u64,
        destination: Memory,
        destination_offset: u64,
        len: u64,
    ) -> Result<()> {
        let source = self.memory_object(source)?;
        let destination = self.memory_object(destination)?;
        let from = self.range(&source, source_offset, len)?;
        let to = self.range(&destination, destination_offset, len)?;
        match (&source, &destination) {
            (MemoryObject::Unshared(source), MemoryObject::Unshared(destination)) => {
                let source = source.data_ptr(&self.inner);
                let destination = destination.data_ptr(&self.inner);
                // SAFETY: both ranges lie inside their memories, which the
                // checks above made against the memories' current sizes. The
                // store is borrowed mutably for this whole method, so nothing
                // else borrows either memory, and nothing grows one between
                // the checks and the copy. `copy` allows the two ranges to
                // overlap, as they do when the two memories are one.
                unsafe {
                    core::ptr::copy(
                        source.add(from.start),
                        destination.add(to.start),
                        from.len(),
                    );
                }
            }
            // A shared memory is reached a byte at a time, atomically, and
            // each byte moves straight from one memory to the other.
            (MemoryObject::Unshared(source), MemoryObject::Shared(destination)) => {
                let cells = &destination.data()[to];
                for (byte, cell) in source.data(&self.inner)[from].iter().zip(cells) {
                    atomic(cell).store(*byte, Ordering::SeqCst);
                }
            }
            (MemoryObject::Shared(source), MemoryObject::Unshared(destination)) => {
                let cells = &source.data()[from];
                for (byte, cell) in destination.data_mut(&mut self.inner)[to]
                    .iter_mut()
                    .zip(cells)
                {
                    *byte = atomic(cell).load(Ordering::SeqCst);
                }
            }
            (MemoryObject::Shared(source), MemoryObject::Shared(destination)) => {
                let from = &source.data()[from];
                let to = &destination.data()[to];
                // The two can be one memory, with ranges that overlap. A copy
                // toward lower addresses runs forward, and one toward higher
                // addresses runs backward, so each byte is read before the
                // copy writes over it.
                let pairs = from.iter().zip(to);
                let copy = |(read, write): (&UnsafeCell<u8>, &UnsafeCell<u8>)| {
                    atomic(write).store(atomic(read).load(Ordering::SeqCst), Ordering::SeqCst);
                };
                if to.as_ptr() <= from.as_ptr() {
                    pairs.for_each(copy);
                } else {
                    pairs.rev().for_each(copy);
                }
            }
        }
        Ok(())
    }

    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
        let types = self.inner.state().types().clone();
        let wasm_ty = convert::to_global_type(&types, &ty)?;
        values::refuse_continuations([wasm_ty.content().clone()])?;
        let mut scope = RootScope::new(&mut self.inner);
        let value = values::to_wasmtime(&mut scope, &value, wasm_ty.content())?;
        let global = wasmtime::Global::new(&mut scope, wasm_ty, value).map_err(type_mismatch)?;
        Ok(scope.as_context_mut().data_mut().add_global(global))
    }

    fn global_ty(&self, global: Global) -> Result<GlobalType> {
        let global = self.inner.state().global(global)?;
        let types = self.inner.state().types();
        Ok(convert::global_type(types, &global.ty(&self.inner)))
    }

    fn global_get(&mut self, global: Global) -> Result<Val> {
        let global = *self.inner.state().global(global)?;
        values::refuse_continuations([global.ty(&self.inner).content().clone()])?;
        let mut scope = RootScope::new(&mut self.inner);
        let value = global.get(&mut scope);
        values::from_wasmtime(&mut scope, &value)
    }

    fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
        let global = *self.inner.state().global(global)?;
        let ty = global.ty(&self.inner);
        if ty.mutability() == wasmtime::Mutability::Const {
            return Err(Error::TypeMismatch {
                message: "the global is immutable".to_string(),
            });
        }
        values::refuse_continuations([ty.content().clone()])?;
        let mut scope = RootScope::new(&mut self.inner);
        let value = values::to_wasmtime(&mut scope, &value, ty.content())?;
        global.set(&mut scope, value).map_err(type_mismatch)
    }

    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
        let types = self.inner.state().types().clone();
        let wasm_ty = convert::to_table_type(&types, &ty)?;
        values::refuse_continuations([wasmtime::ValType::Ref(wasm_ty.element().clone())])?;
        let mut scope = RootScope::new(&mut self.inner);
        let init = values::to_wasmtime_ref(&mut scope, &init, wasm_ty.element())?;
        let table = wasmtime::Table::new(&mut scope, wasm_ty, init).map_err(errors::backend)?;
        Ok(scope.as_context_mut().data_mut().add_table(table))
    }

    fn table_ty(&self, table: Table) -> Result<TableType> {
        let table = self.inner.state().table(table)?;
        let types = self.inner.state().types();
        Ok(convert::table_type(types, &table.ty(&self.inner)))
    }

    fn table_size(&self, table: Table) -> Result<u64> {
        Ok(self.inner.state().table(table)?.size(&self.inner))
    }

    fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
        let table = *self.inner.state().table(table)?;
        let element = table.ty(&self.inner).element().clone();
        values::refuse_continuations([wasmtime::ValType::Ref(element)])?;
        let mut scope = RootScope::new(&mut self.inner);
        let size = table.size(&scope);
        let element = table
            .get(&mut scope, index)
            .ok_or(Error::TableOutOfBounds { index, size })?;
        values::from_wasmtime(&mut scope, &element.into())
    }

    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
        let table = *self.inner.state().table(table)?;
        let size = table.size(&self.inner);
        if index >= size {
            return Err(Error::TableOutOfBounds { index, size });
        }
        let element = table.ty(&self.inner).element().clone();
        values::refuse_continuations([wasmtime::ValType::Ref(element.clone())])?;
        let mut scope = RootScope::new(&mut self.inner);
        let value = values::to_wasmtime_ref(&mut scope, &value, &element)?;
        table.set(&mut scope, index, value).map_err(type_mismatch)
    }

    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
        let table = *self.inner.state().table(table)?;
        let element = table.ty(&self.inner).element().clone();
        values::refuse_continuations([wasmtime::ValType::Ref(element.clone())])?;
        let mut scope = RootScope::new(&mut self.inner);
        let init = values::to_wasmtime_ref(&mut scope, &init, &element)?;
        table
            .grow(&mut scope, delta, init)
            .map_err(|_| Error::Grow { delta })
    }

    fn tag_ty(&self, tag: Tag) -> Result<TagType> {
        let tag = self.inner.state().tag(tag)?;
        let types = self.inner.state().types();
        Ok(convert::tag_type(types, &tag.ty(&self.inner)))
    }

    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
        let mut scope = RootScope::new(&mut self.inner);
        let extern_ref = wasmtime::ExternRef::new(&mut scope, value)
            .and_then(|extern_ref| extern_ref.to_owned_rooted(&mut scope))
            .map_err(errors::backend)?;
        Ok(scope.as_context_mut().data_mut().add_extern_ref(extern_ref))
    }

    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
        let extern_ref = self.inner.state().extern_ref(extern_ref)?;
        let data = extern_ref
            .data(self.inner.as_context())
            .map_err(errors::backend)?
            .ok_or_else(|| Error::Backend {
                message: "the externref wraps an internal reference, not a value the host made"
                    .to_string(),
            })?;
        // `extern_ref_new` boxes every value the host makes, so the data of
        // an `externref` the host made is that box.
        data.downcast_ref::<Box<dyn Any + Send + Sync>>()
            .map(|value| &**value)
            .ok_or_else(|| Error::Backend {
                message: "the externref holds a value of another embedder".to_string(),
            })
    }

    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        let mut scope = RootScope::new(&mut self.inner);
        let any_ref =
            wasmtime::AnyRef::from_i31(&mut scope, wasmtime::I31::wrapping_u32(value.get_u32()))
                .to_owned_rooted(&mut scope)
                .map_err(errors::backend)?;
        Ok(scope.as_context_mut().data_mut().add_any_ref(any_ref))
    }

    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        let any_ref = self.inner.state().any_ref(any_ref)?;
        Ok(any_ref
            .as_i31(&self.inner)
            .map_err(errors::backend)?
            .map(|value| I31::wrapping_u32(value.get_u32())))
    }
}

/// Runs the host function `func` for a call from a guest, in the store the
/// guest runs in.
///
/// Each call has its own arguments and its own result slots, which start
/// as `slots`. An error of the host function, or a result it gave that
/// does not fit `result_types`, leaves as a [`HostError`], which the call
/// that ran the guest turns back into the host's own error.
fn call_host(
    func: &HostFunc,
    slots: &[Val],
    result_types: &[wasmtime::ValType],
    caller: wasmtime::Caller<'_, crate::state::State>,
    params: &[wasmtime::Val],
    results: &mut [wasmtime::Val],
) -> wasmtime::Result<()> {
    let host = |error: anyhow::Error| wasmtime::Error::new(HostError(error));
    let mut store = WasmtimeStore::new(caller);
    let params = params
        .iter()
        .map(|value| values::from_wasmtime(&mut store.inner, value))
        .collect::<Result<Vec<_>>>()
        .map_err(|error| host(error.into()))?;
    let mut outputs = slots.to_vec();
    match func.call(&mut store, &params, &mut outputs) {
        Ok(Poll::Ready(())) => {}
        Ok(Poll::Pending) => {
            return Err(host(anyhow::anyhow!(
                "a host function that cannot suspend answered \"not yet\""
            )));
        }
        Err(error) => return Err(host(error)),
    }
    for ((slot, output), ty) in results.iter_mut().zip(&outputs).zip(result_types) {
        *slot = values::to_wasmtime(&mut store.inner, output, ty)
            .map_err(|error| host(error.into()))?;
    }
    Ok(())
}

/// [`Error::TypeMismatch`], with Wasmtime's words for `error`.
fn type_mismatch(error: wasmtime::Error) -> Error {
    Error::TypeMismatch {
        message: format!("{error:#}"),
    }
}

/// The byte of a shared memory in `cell`, as an atomic.
fn atomic(cell: &UnsafeCell<u8>) -> &AtomicU8 {
    // SAFETY: the cell is a byte of a shared memory that Wasmtime lends for
    // as long as the memory lives. A byte is always aligned for `AtomicU8`.
    // Wasmtime requires every access to the bytes of a shared memory to be
    // atomic, and every access this backend makes goes through here.
    unsafe { AtomicU8::from_ptr(cell.get()) }
}
