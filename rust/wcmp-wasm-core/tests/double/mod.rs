//! Three test doubles of a backend, for the tests of the engine's own
//! plumbing.
//!
//! `Double` keeps every object of a store in a vector, and runs no
//! WebAssembly: its only functions are host functions. It "compiles" a
//! module from a line of text, `imports N`, into a module that imports `N`
//! memories and exports each again as `m0`, `m1`, and so on. A host function
//! receives the store wrapped in a `DoubleCaller`, a type of its own that
//! borrows the store, as a Wasmi or Wasmtime backend wraps its own caller.
//! `Minimal` is `Double` with every default body of `BackendStore` left in
//! place, and a memory that never lends its bytes. `Refuser` refuses
//! everything. They are different types, so a test that holds two holds two
//! backends in one binary.

use std::any::Any;
use std::task::Poll;

use wcmp_wasm_core::backend::{
    Backend, BackendModule, BackendStore, BackendSuspendedCall, BoxFuture, HostFunc, RawHandle,
    StoreData,
};
use wcmp_wasm_core::{
    AnyRef, Capabilities, Error, ExportType, Extern, ExternRef, ExternType, Func, FuncType, Global,
    GlobalType, I31, ImportType, Instance, Memory, MemoryType, Mutability, Result, ResumableCall,
    SuspendedCall, Table, TableType, Tag, TagType, TrapKind, Val,
};

/// A backend over vectors, which declares `capabilities`.
pub struct Double {
    pub capabilities: Capabilities,
}

impl Backend for Double {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        Box::pin(async move { self.compile_sync(bytes) })
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        let text = std::str::from_utf8(bytes).map_err(|error| Error::Compile {
            message: error.to_string(),
        })?;
        let count = text
            .strip_prefix("imports ")
            .and_then(|count| count.parse::<usize>().ok())
            .ok_or_else(|| Error::Compile {
                message: format!("the double cannot read `{text}`"),
            })?;
        let ty = ExternType::Memory(MemoryType::new(1, None));
        Ok(Box::new(DoubleModule {
            imports: (0..count)
                .map(|index| ImportType::new("host", format!("m{index}"), ty.clone()))
                .collect(),
            exports: (0..count)
                .map(|index| ExportType::new(format!("m{index}"), ty.clone()))
                .collect(),
        }))
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        Ok(Box::new(DoubleStore::new(data)))
    }
}

/// `Double`, with every default body of `BackendStore` left in place, and
/// a memory that never lends its bytes. It declares `capabilities`.
pub struct Minimal {
    pub capabilities: Capabilities,
}

impl Backend for Minimal {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        Box::pin(async move { self.compile_sync(bytes) })
    }

    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>> {
        Double {
            capabilities: self.capabilities,
        }
        .compile_sync(bytes)
    }

    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>> {
        Ok(Box::new(MinimalStore(DoubleStore::new(data))))
    }
}

/// A backend that declares nothing and refuses everything.
pub struct Refuser;

impl Backend for Refuser {
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }

    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>> {
        Box::pin(async move { self.compile_sync(bytes) })
    }

    fn compile_sync(&self, _: &[u8]) -> Result<Box<dyn BackendModule>> {
        Err(Error::Compile {
            message: "the refuser compiles nothing".to_string(),
        })
    }

    fn new_store(&self, _: StoreData) -> Result<Box<dyn BackendStore>> {
        Err(Error::Backend {
            message: "the refuser makes no store".to_string(),
        })
    }
}

struct DoubleModule {
    imports: Vec<ImportType>,
    exports: Vec<ExportType>,
}

impl BackendModule for DoubleModule {
    fn imports(&self) -> &[ImportType] {
        &self.imports
    }

    fn exports(&self) -> &[ExportType] {
        &self.exports
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct DoubleStore {
    data: StoreData,
    instances: Vec<Vec<(String, Extern)>>,
    funcs: Vec<(FuncType, HostFunc)>,
    memories: Vec<(MemoryType, Vec<u8>)>,
    globals: Vec<(GlobalType, Val)>,
    tables: Vec<(TableType, Vec<Val>)>,
    extern_refs: Vec<Box<dyn Any + Send + Sync>>,
    i31s: Vec<I31>,
    /// The state of each call that waits: the suspending function it waits
    /// in. A resumption takes its state from here.
    waiting: Vec<Option<Func>>,
}

/// The slot of `handle` in `objects`, where the store knows it.
fn slot<H: RawHandle>(handle: H) -> Result<usize> {
    usize::try_from(handle.index()).map_err(|_| Error::WrongStore)
}

fn get<H: RawHandle, O>(objects: &[O], handle: H) -> Result<&O> {
    objects.get(slot(handle)?).ok_or(Error::WrongStore)
}

fn get_mut<H: RawHandle, O>(objects: &mut [O], handle: H) -> Result<&mut O> {
    objects.get_mut(slot(handle)?).ok_or(Error::WrongStore)
}

/// The range of `len` bytes at `offset` in `bytes`, where it fits.
fn range(bytes: &[u8], offset: u64, len: usize) -> Result<std::ops::Range<usize>> {
    let out_of_bounds = || Error::MemoryOutOfBounds {
        offset,
        len: len as u64,
        size: bytes.len() as u64,
    };
    let start = usize::try_from(offset).map_err(|_| out_of_bounds())?;
    let end = start.checked_add(len).ok_or_else(out_of_bounds)?;
    if end <= bytes.len() {
        Ok(start..end)
    } else {
        Err(out_of_bounds())
    }
}

impl DoubleStore {
    fn handle<H: RawHandle>(&self, index: usize) -> H {
        H::from_raw(self.data.id(), index as u64)
    }

    fn new(data: StoreData) -> Self {
        Self {
            data,
            instances: Vec::new(),
            funcs: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            tables: Vec::new(),
            extern_refs: Vec::new(),
            i31s: Vec::new(),
            waiting: Vec::new(),
        }
    }

    fn call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<Poll<()>> {
        let body = get(&self.funcs, func)?.1.clone();
        body.call(&mut DoubleCaller(self), params, results)
            .map_err(|error| Error::Trap(TrapKind::Host(error)))
    }

    /// Resumes `call`, which waits in this store. The resumption hands the
    /// results of the suspending function straight back as the results of
    /// the call, once it checks them against the type of the function it
    /// finds in the store.
    fn resume(
        &mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &[Val],
        results: &mut [Val],
    ) -> Result<ResumableCall> {
        let call = call
            .into_any()
            .downcast::<DoubleSuspension>()
            .map_err(|_| Error::WrongStore)?;
        let func = self
            .waiting
            .get_mut(call.slot)
            .and_then(Option::take)
            .ok_or(Error::WrongStore)?;
        let arity = get(&self.funcs, func)?.0.results().len();
        if import_results.len() != arity || results.len() != arity {
            return Err(Error::TypeMismatch {
                message: format!(
                    "the resumption gave {} results to a function of {arity}",
                    import_results.len()
                ),
            });
        }
        results.copy_from_slice(import_results);
        Ok(ResumableCall::Finished)
    }
}

impl BackendStore for DoubleStore {
    fn data(&self) -> &StoreData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut StoreData {
        &mut self.data
    }

    fn instantiate<'a>(
        &'a mut self,
        module: &'a dyn BackendModule,
        imports: &'a [Extern],
    ) -> BoxFuture<'a, Result<Instance>> {
        Box::pin(async move {
            let module = module
                .as_any()
                .downcast_ref::<DoubleModule>()
                .ok_or(Error::WrongEngine)?;
            let exports = module
                .exports
                .iter()
                .zip(imports)
                .map(|(export, import)| (export.name().to_string(), *import))
                .collect();
            self.instances.push(exports);
            Ok(self.handle(self.instances.len() - 1))
        })
    }

    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
        Ok(get(&self.instances, instance)?
            .iter()
            .find(|(export, _)| export == name)
            .map(|(_, external)| *external))
    }

    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
        self.funcs.push((ty, func));
        Ok(self.handle(self.funcs.len() - 1))
    }

    fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
        Ok(Some(get(&self.funcs, func)?.0.clone()))
    }

    fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()> {
        match self.call(func, params, results)? {
            Poll::Ready(()) => Ok(()),
            Poll::Pending => Err(Error::Trap(TrapKind::Other(
                "a host function suspended outside a resumable call".to_string(),
            ))),
        }
    }

    fn func_call_resumable<'a>(
        &'a mut self,
        func: Func,
        params: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(async move {
            Ok(match self.call(func, params, results)? {
                Poll::Ready(()) => ResumableCall::Finished,
                Poll::Pending => {
                    self.waiting.push(Some(func));
                    ResumableCall::Suspended(SuspendedCall::new(
                        self.data.id(),
                        Box::new(DoubleSuspension {
                            slot: self.waiting.len() - 1,
                        }),
                    ))
                }
            })
        })
    }

    fn resume_call<'a>(
        &'a mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(std::future::ready(self.resume(
            call,
            import_results,
            results,
        )))
    }

    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
        let size =
            usize::try_from(ty.minimum() * MemoryType::PAGE_SIZE).map_err(|_| Error::Grow {
                delta: ty.minimum(),
            })?;
        self.memories.push((ty, vec![0; size]));
        Ok(self.handle(self.memories.len() - 1))
    }

    fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
        Ok(get(&self.memories, memory)?.0)
    }

    fn memory_size(&self, memory: Memory) -> Result<u64> {
        Ok(get(&self.memories, memory)?.1.len() as u64)
    }

    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
        let (ty, bytes) = get_mut(&mut self.memories, memory)?;
        let old = bytes.len() as u64 / MemoryType::PAGE_SIZE;
        let new = old.checked_add(pages).ok_or(Error::Grow { delta: pages })?;
        if ty.maximum().is_some_and(|maximum| new > maximum) {
            return Err(Error::Grow { delta: pages });
        }
        let size = usize::try_from(new * MemoryType::PAGE_SIZE)
            .map_err(|_| Error::Grow { delta: pages })?;
        bytes.resize(size, 0);
        Ok(old)
    }

    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let bytes = &get(&self.memories, memory)?.1;
        buffer.copy_from_slice(&bytes[range(bytes, offset, buffer.len())?]);
        Ok(())
    }

    fn memory_write(&mut self, memory: Memory, offset: u64, source: &[u8]) -> Result<()> {
        let bytes = &mut get_mut(&mut self.memories, memory)?.1;
        let range = range(bytes, offset, source.len())?;
        bytes[range].copy_from_slice(source);
        Ok(())
    }

    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        let bytes = &get(&self.memories, memory)?.1;
        f(&bytes[range(bytes, offset, len)?]);
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
        let len = usize::try_from(len).map_err(|_| Error::MemoryOutOfBounds {
            offset: source_offset,
            len,
            size: 0,
        })?;
        let from = &get(&self.memories, source)?.1;
        let copied = from[range(from, source_offset, len)?].to_vec();
        self.memory_write(destination, destination_offset, &copied)
    }

    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
        self.globals.push((ty, value));
        Ok(self.handle(self.globals.len() - 1))
    }

    fn global_ty(&self, global: Global) -> Result<GlobalType> {
        Ok(get(&self.globals, global)?.0)
    }

    fn global_get(&mut self, global: Global) -> Result<Val> {
        Ok(get(&self.globals, global)?.1)
    }

    fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
        let (ty, slot) = get_mut(&mut self.globals, global)?;
        if ty.mutability() == Mutability::Const {
            return Err(Error::TypeMismatch {
                message: "the global is immutable".to_string(),
            });
        }
        *slot = value;
        Ok(())
    }

    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
        let size = usize::try_from(ty.minimum()).map_err(|_| Error::Grow {
            delta: ty.minimum(),
        })?;
        self.tables.push((ty, vec![init; size]));
        Ok(self.handle(self.tables.len() - 1))
    }

    fn table_ty(&self, table: Table) -> Result<TableType> {
        Ok(get(&self.tables, table)?.0)
    }

    fn table_size(&self, table: Table) -> Result<u64> {
        Ok(get(&self.tables, table)?.1.len() as u64)
    }

    fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
        let elements = &get(&self.tables, table)?.1;
        usize::try_from(index)
            .ok()
            .and_then(|index| elements.get(index))
            .copied()
            .ok_or(Error::TableOutOfBounds {
                index,
                size: elements.len() as u64,
            })
    }

    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
        let elements = &mut get_mut(&mut self.tables, table)?.1;
        let size = elements.len() as u64;
        let element = usize::try_from(index)
            .ok()
            .and_then(|index| elements.get_mut(index))
            .ok_or(Error::TableOutOfBounds { index, size })?;
        *element = value;
        Ok(())
    }

    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
        let elements = &mut get_mut(&mut self.tables, table)?.1;
        let old = elements.len() as u64;
        let delta_len = usize::try_from(delta).map_err(|_| Error::Grow { delta })?;
        elements.extend(std::iter::repeat_n(init, delta_len));
        Ok(old)
    }

    fn tag_ty(&self, _: Tag) -> Result<TagType> {
        // The double makes no tag, so every tag handle is foreign to it.
        Err(Error::WrongStore)
    }

    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
        self.extern_refs.push(value);
        Ok(self.handle(self.extern_refs.len() - 1))
    }

    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
        Ok(&**get(&self.extern_refs, extern_ref)?)
    }

    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        self.i31s.push(value);
        Ok(self.handle(self.i31s.len() - 1))
    }

    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        Ok(Some(*get(&self.i31s, any_ref)?))
    }
}

/// A call of the double that waits. It holds only the slot of its state in
/// the store, so it resumes only through the store.
struct DoubleSuspension {
    slot: usize,
}

impl BackendSuspendedCall for DoubleSuspension {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// Implements every required method of `BackendStore` for a type whose
/// field `0` reaches a `DoubleStore`, by handing the method to that store,
/// and adds the methods in the braces.
macro_rules! delegate {
    ($store:ty { $($methods:tt)* }) => {
        impl BackendStore for $store {
            fn data(&self) -> &StoreData {
                self.0.data()
            }

            fn data_mut(&mut self) -> &mut StoreData {
                self.0.data_mut()
            }

            fn instantiate<'a>(
                &'a mut self,
                module: &'a dyn BackendModule,
                imports: &'a [Extern],
            ) -> BoxFuture<'a, Result<Instance>> {
                self.0.instantiate(module, imports)
            }

            fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
                self.0.instance_export(instance, name)
            }

            fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
                self.0.func_new(ty, func)
            }

            fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
                self.0.func_ty(func)
            }

            fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()> {
                self.0.func_call(func, params, results)
            }

            fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
                self.0.memory_new(ty)
            }

            fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
                self.0.memory_ty(memory)
            }

            fn memory_size(&self, memory: Memory) -> Result<u64> {
                self.0.memory_size(memory)
            }

            fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
                self.0.memory_grow(memory, pages)
            }

            fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
                self.0.memory_read(memory, offset, buffer)
            }

            fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()> {
                self.0.memory_write(memory, offset, bytes)
            }

            fn memory_copy(
                &mut self,
                source: Memory,
                source_offset: u64,
                destination: Memory,
                destination_offset: u64,
                len: u64,
            ) -> Result<()> {
                self.0
                    .memory_copy(source, source_offset, destination, destination_offset, len)
            }

            fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
                self.0.global_new(ty, value)
            }

            fn global_ty(&self, global: Global) -> Result<GlobalType> {
                self.0.global_ty(global)
            }

            fn global_get(&mut self, global: Global) -> Result<Val> {
                self.0.global_get(global)
            }

            fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
                self.0.global_set(global, value)
            }

            fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
                self.0.table_new(ty, init)
            }

            fn table_ty(&self, table: Table) -> Result<TableType> {
                self.0.table_ty(table)
            }

            fn table_size(&self, table: Table) -> Result<u64> {
                self.0.table_size(table)
            }

            fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
                self.0.table_get(table, index)
            }

            fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
                self.0.table_set(table, index, value)
            }

            fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
                self.0.table_grow(table, delta, init)
            }

            fn tag_ty(&self, tag: Tag) -> Result<TagType> {
                self.0.tag_ty(tag)
            }

            fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
                self.0.extern_ref_new(value)
            }

            fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
                self.0.extern_ref_data(extern_ref)
            }

            $($methods)*
        }
    };
}

/// The store a host function of the double receives while a call runs: a
/// type of its own, which borrows the store, as a backend's wrapper around
/// its engine's caller does. It resumes a waiting call through the store it
/// borrows, so a host function can resume a call.
struct DoubleCaller<'a>(&'a mut DoubleStore);

delegate!(DoubleCaller<'_> {
    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        self.0.memory_with_bytes(memory, offset, len, f)
    }

    fn func_call_resumable<'a>(
        &'a mut self,
        func: Func,
        params: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        self.0.func_call_resumable(func, params, results)
    }

    fn resume_call<'a>(
        &'a mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(std::future::ready(self.0.resume(call, import_results, results)))
    }

    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        self.0.any_ref_from_i31(value)
    }

    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        self.0.any_ref_as_i31(any_ref)
    }
});

/// The store of `Minimal`: a `DoubleStore` with every default body of
/// `BackendStore` left in place.
struct MinimalStore(DoubleStore);

delegate!(MinimalStore {
    /// Answers without lending the bytes, which breaks the contract, so a
    /// test can watch the engine catch it.
    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        _: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        self.0.memory_with_bytes(memory, offset, len, &mut |_| ())
    }
});
