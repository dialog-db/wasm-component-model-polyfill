//! A store of the Wasmi backend, as the runtime layer reaches it.

use core::any::Any;
use core::ops::Range;
use core::task::Poll;

use wcmp_wasm_core::backend::{BackendModule, BackendStore, BoxFuture, HostFunc, StoreData};
use wcmp_wasm_core::{
    Capability, Error, Extern, ExternRef, Func, FuncType, Global, GlobalType, Instance, Memory,
    MemoryType, Result, Table, TableType, Tag, TagType, Val,
};

use crate::context::Context;
use crate::convert;
use crate::errors;
use crate::host_error::HostError;
use crate::module::WasmiModule;
use crate::state::State;
use crate::values;

/// A store of the Wasmi backend, over a Wasmi context `C`.
///
/// The context is the Wasmi store itself, which the runtime layer's
/// `Store` owns, or the `Caller` a host function receives while a guest
/// runs in the store. One implementation serves both, so a host function
/// reaches the store exactly as the host does, and can call back into the
/// guest, as deep as `MAX_HOST_DEPTH` allows.
pub struct WasmiStore<C> {
    inner: C,
}

impl<C: Context> WasmiStore<C> {
    /// The store over the Wasmi context `inner`.
    pub fn new(inner: C) -> Self {
        Self { inner }
    }

    /// Instantiates `module` with `imports`, synchronously, as Wasmi does.
    fn instantiate_now(
        &mut self,
        module: &dyn BackendModule,
        imports: &[Extern],
    ) -> Result<Instance> {
        let module = module
            .as_any()
            .downcast_ref::<WasmiModule>()
            .ok_or(Error::WrongEngine)?;
        if !wasmi::Engine::same(module.module().engine(), self.inner.as_context().engine()) {
            return Err(Error::WrongEngine);
        }
        let order = module.link_order();
        if imports.len() != order.len() {
            return Err(Error::ImportCount {
                expected: order.len(),
                actual: imports.len(),
            });
        }
        // The imports come in the order the module declares them, and Wasmi
        // takes them in its own.
        let imports = order
            .iter()
            .map(|declared| self.inner.state().to_extern(&imports[*declared]))
            .collect::<Result<Vec<_>>>()?;
        let instance = wasmi::Instance::new(&mut self.inner, module.module(), &imports)
            .map_err(errors::instantiation)?;
        Ok(self.inner.state_mut().add_instance(instance))
    }

    /// The memory that `memory` names.
    fn memory_object(&self, memory: Memory) -> Result<wasmi::Memory> {
        self.inner.state().memory(memory).copied()
    }

    /// The range of the `len` bytes at `offset` of `memory`, where the
    /// memory holds them all.
    fn range(&self, memory: wasmi::Memory, offset: u64, len: u64) -> Result<Range<usize>> {
        let size = memory.data_size(&self.inner) as u64;
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
}

impl<C: Context> BackendStore for WasmiStore<C> {
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
        // Wasmi instantiates synchronously, so the future is ready on its
        // first poll.
        Box::pin(core::future::ready(self.instantiate_now(module, imports)))
    }

    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
        let instance = *self.inner.state().instance(instance)?;
        Ok(instance
            .get_export(&self.inner, name)
            .map(|export| self.inner.state_mut().add_extern(export)))
    }

    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
        if func.is_suspending() {
            return Err(Error::Unsupported(Capability::HostSuspension));
        }
        let wasm_ty = convert::to_func_type(&ty)?;
        let result_types = wasm_ty.results().to_vec();
        let slots = ty
            .results()
            .iter()
            .map(|ty| Val::default_for_ty(ty).unwrap_or(Val::I32(0)))
            .collect::<Vec<_>>();
        let host = wasmi::Func::new(&mut self.inner, wasm_ty, move |caller, params, results| {
            call_host(&func, &slots, &result_types, caller, params, results)
        });
        Ok(self.inner.state_mut().add_func(host))
    }

    fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
        let func = self.inner.state().func(func)?;
        Ok(Some(convert::func_type(&func.ty(&self.inner))))
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
        let arguments = params
            .iter()
            .zip(ty.params())
            .map(|(value, ty)| values::to_wasmi(self.inner.state(), value, *ty))
            .collect::<Result<Vec<_>>>()?;
        let mut outputs = ty
            .results()
            .iter()
            .map(|ty| wasmi::Val::default_for_ty(*ty))
            .collect::<Vec<_>>();
        func.call(&mut self.inner, &arguments, &mut outputs)
            .map_err(errors::trap)?;
        let state = self.inner.state_mut();
        for (slot, output) in results.iter_mut().zip(&outputs) {
            *slot = values::from_wasmi(state, output);
        }
        Ok(())
    }

    // `func_call_resumable` and `resume_call` keep the contract's bodies,
    // which return `Unsupported(host_suspension)`: see the crate's
    // documentation of its capabilities.

    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
        let wasm_ty = convert::to_memory_type(&ty)?;
        let memory = wasmi::Memory::new(&mut self.inner, wasm_ty).map_err(errors::backend)?;
        Ok(self.inner.state_mut().add_memory(memory))
    }

    fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
        let memory = self.memory_object(memory)?;
        Ok(convert::memory_type(memory.ty(&self.inner)))
    }

    fn memory_size(&self, memory: Memory) -> Result<u64> {
        let memory = self.memory_object(memory)?;
        Ok(memory.data_size(&self.inner) as u64)
    }

    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
        let memory = self.memory_object(memory)?;
        // The backend makes no memory whose growth Wasmi panics on, but a
        // guest may declare one and export it, so the host's growth of such
        // a memory fails here before it reaches Wasmi.
        if pages > 0 && !convert::grows_safely(memory.ty(&self.inner)) {
            return Err(Error::Grow { delta: pages });
        }
        memory
            .grow(&mut self.inner, pages)
            .map_err(|_| Error::Grow { delta: pages })
    }

    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let memory = self.memory_object(memory)?;
        let range = self.range(memory, offset, buffer.len() as u64)?;
        buffer.copy_from_slice(&memory.data(&self.inner)[range]);
        Ok(())
    }

    fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()> {
        let memory = self.memory_object(memory)?;
        let range = self.range(memory, offset, bytes.len() as u64)?;
        memory.data_mut(&mut self.inner)[range].copy_from_slice(bytes);
        Ok(())
    }

    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        let memory = self.memory_object(memory)?;
        let range = self.range(memory, offset, len as u64)?;
        // Wasmi has no shared memory, so every memory lends its own bytes.
        f(&memory.data(&self.inner)[range]);
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
        let from = self.range(source, source_offset, len)?;
        let to = self.range(destination, destination_offset, len)?;
        let source = source.data_ptr(&self.inner);
        let destination = destination.data_ptr(&self.inner);
        // SAFETY: both ranges lie inside their memories, which the checks
        // above made against the memories' current sizes. The store is
        // borrowed mutably for this whole method, so nothing else borrows
        // either memory, and nothing grows one between the checks and the
        // copy. `copy` allows the two ranges to overlap, as they do when the
        // two memories are one.
        unsafe {
            core::ptr::copy(
                source.add(from.start),
                destination.add(to.start),
                from.len(),
            );
        }
        Ok(())
    }

    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
        let content = convert::to_val_type(ty.content())?;
        let value = values::to_wasmi(self.inner.state(), &value, content)?;
        let global = wasmi::Global::new(
            &mut self.inner,
            value,
            convert::to_mutability(ty.mutability()),
        );
        Ok(self.inner.state_mut().add_global(global))
    }

    fn global_ty(&self, global: Global) -> Result<GlobalType> {
        let global = self.inner.state().global(global)?;
        Ok(convert::global_type(global.ty(&self.inner)))
    }

    fn global_get(&mut self, global: Global) -> Result<Val> {
        let global = *self.inner.state().global(global)?;
        let value = global.get(&self.inner);
        Ok(values::from_wasmi(self.inner.state_mut(), &value))
    }

    fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
        let global = *self.inner.state().global(global)?;
        let ty = global.ty(&self.inner);
        if ty.mutability() == wasmi::Mutability::Const {
            return Err(Error::TypeMismatch {
                message: "the global is immutable".to_string(),
            });
        }
        let value = values::to_wasmi(self.inner.state(), &value, ty.content())?;
        global
            .set(&mut self.inner, value)
            .map_err(|error| Error::TypeMismatch {
                message: error.to_string(),
            })
    }

    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
        let wasm_ty = convert::to_table_type(&ty)?;
        let init = values::to_wasmi_ref(self.inner.state(), &init, wasm_ty.element())?;
        let table = wasmi::Table::new(&mut self.inner, wasm_ty, init).map_err(errors::backend)?;
        Ok(self.inner.state_mut().add_table(table))
    }

    fn table_ty(&self, table: Table) -> Result<TableType> {
        let table = self.inner.state().table(table)?;
        Ok(convert::table_type(table.ty(&self.inner)))
    }

    fn table_size(&self, table: Table) -> Result<u64> {
        Ok(self.inner.state().table(table)?.size(&self.inner))
    }

    fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
        let table = *self.inner.state().table(table)?;
        let size = table.size(&self.inner);
        let element = table
            .get(&self.inner, index)
            .ok_or(Error::TableOutOfBounds { index, size })?;
        Ok(values::from_wasmi_ref(self.inner.state_mut(), &element))
    }

    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
        let table = *self.inner.state().table(table)?;
        let size = table.size(&self.inner);
        if index >= size {
            return Err(Error::TableOutOfBounds { index, size });
        }
        let element = table.ty(&self.inner).element();
        let value = values::to_wasmi_ref(self.inner.state(), &value, element)?;
        table
            .set(&mut self.inner, index, value)
            .map_err(|error| Error::TypeMismatch {
                message: error.to_string(),
            })
    }

    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
        let table = *self.inner.state().table(table)?;
        let element = table.ty(&self.inner).element();
        let init = values::to_wasmi_ref(self.inner.state(), &init, element)?;
        table
            .grow(&mut self.inner, delta, init)
            .map_err(|_| Error::Grow { delta })
    }

    fn tag_ty(&self, tag: Tag) -> Result<TagType> {
        // Wasmi has no tags, so no store of this backend makes one: the
        // handle names nothing here.
        let _ = tag;
        Err(Error::Unsupported(Capability::Exceptions))
    }

    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
        let extern_ref = wasmi::ExternRef::new(&mut self.inner, value);
        Ok(self.inner.state_mut().add_extern_ref(extern_ref))
    }

    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
        let extern_ref = self.inner.state().extern_ref(extern_ref)?;
        // `extern_ref_new` boxes every value the host makes, so the data of
        // an `externref` the host made is that box.
        extern_ref
            .data(&self.inner)
            .downcast_ref::<Box<dyn Any + Send + Sync>>()
            .map(|value| &**value)
            .ok_or_else(|| Error::Backend {
                message: "the externref holds a value of another embedder".to_string(),
            })
    }
}

/// The most calls of host functions that run in one store, each inside the
/// last.
///
/// Wasmi keeps a guest's own calls on a stack of its own, and bounds them.
/// A call from a guest into a host function that calls back into a guest
/// runs Wasmi again on the native stack, and Wasmi bounds only the calls
/// of each run, so nothing of Wasmi's bounds how deep that goes. Each such
/// round trip takes about 3 KiB of the native stack in an optimized build,
/// and up to 16 KiB where nothing is optimized. At this bound, the deepest
/// descent takes at most about 1 MiB, which leaves room in the 2 MiB stack
/// that Rust gives a new thread by default.
const MAX_HOST_DEPTH: u32 = 64;

/// Runs the host function `func` for a call from a guest, in the store the
/// guest runs in.
///
/// Each call has its own arguments and its own result slots, which start
/// as `slots`. An error of the host function, or a result it gave that
/// does not fit `result_types`, leaves as a [`HostError`], which the call
/// that ran the guest turns back into the host's own error. A call beyond
/// [`MAX_HOST_DEPTH`] traps with a stack overflow instead of running the
/// host function.
fn call_host(
    func: &HostFunc,
    slots: &[Val],
    result_types: &[wasmi::ValType],
    caller: wasmi::Caller<'_, State>,
    params: &[wasmi::Val],
    results: &mut [wasmi::Val],
) -> core::result::Result<(), wasmi::Error> {
    let mut store = WasmiStore::new(caller);
    let depth = store.inner.state().host_depth();
    if depth >= MAX_HOST_DEPTH {
        return Err(wasmi::Error::from(wasmi::TrapCode::StackOverflow));
    }
    store.inner.state_mut().set_host_depth(depth + 1);
    let outcome = run_host(func, slots, result_types, &mut store, params, results);
    store.inner.state_mut().set_host_depth(depth);
    outcome
}

/// Runs the host function `func` in `store`, for [`call_host`].
fn run_host(
    func: &HostFunc,
    slots: &[Val],
    result_types: &[wasmi::ValType],
    store: &mut WasmiStore<wasmi::Caller<'_, State>>,
    params: &[wasmi::Val],
    results: &mut [wasmi::Val],
) -> core::result::Result<(), wasmi::Error> {
    let host = |error: anyhow::Error| wasmi::Error::host(HostError(error));
    let params = params
        .iter()
        .map(|value| values::from_wasmi(store.inner.state_mut(), value))
        .collect::<Vec<_>>();
    let mut outputs = slots.to_vec();
    match func.call(store, &params, &mut outputs) {
        Ok(Poll::Ready(())) => {}
        Ok(Poll::Pending) => {
            return Err(host(anyhow::anyhow!(
                "a host function that cannot suspend answered \"not yet\""
            )));
        }
        Err(error) => return Err(host(error)),
    }
    for ((slot, output), ty) in results.iter_mut().zip(&outputs).zip(result_types) {
        *slot = values::to_wasmi(store.inner.state(), output, *ty)
            .map_err(|error| host(error.into()))?;
    }
    Ok(())
}
