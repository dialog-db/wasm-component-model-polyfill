// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The store of the browser backend, as its host holds it.

use core::any::Any;
use std::rc::Rc;

use wcmp_wasm_core::backend::{
    BackendModule, BackendResumption, BackendStore, BackendSuspendedCall, BoxFuture, HostFunc,
    StoreData, StoreId,
};
use wcmp_wasm_core::{
    AnyRef, Engine, Error, Extern, ExternRef, Func, FuncType, Global, GlobalType, I31, Instance,
    Memory, MemoryType, Result, ResumableCall, Resumption, Table, TableType, Tag, TagType, Val,
};

use crate::calls::Calls;
use crate::cell::StoreCell;
use crate::resumption::WebResumption;
use crate::store::WebStore;
use crate::suspended::WebSuspendedCall;

/// The store of the browser backend, as the host holds it through
/// [`Store`](wcmp_wasm_core::Store): the owner of the store's cell.
///
/// A flight of the store runs on a microtask, and reaches the store
/// through the cell while its permit holds. So each method here first moves
/// the store's epoch on, which ends every permit, and only then reaches the
/// store. A flight that runs after the host took the store back, because
/// the future that waited for it dropped or the host used the store since,
/// finds its permit gone and does not reach the store: a resumed stack
/// parks until a wait takes it up again, and a start function traps.
///
/// A forgotten future of the owner no longer borrows the store, yet its
/// flight keeps its permit, so the flight's host function can hold the
/// store by a global and reach it through the owner while the flight's own
/// reference to the store lives. Each method here refuses the store then:
/// a fallible one with [`Error::Backend`], and [`data`](BackendStore::data)
/// and [`data_mut`](BackendStore::data_mut), which cannot fail, with a
/// panic. The owner keeps the store's identity and engine outside the
/// cell, so [`id`](BackendStore::id) and [`engine`](BackendStore::engine)
/// answer without the store, and the engine's checks of a handle never
/// meet the panic.
///
/// When the owner drops, the store drops with it, unless a flight runs.
/// A flight that runs keeps the cell until it stops, and reaches the store
/// until then, since nothing else can.
pub struct Owner {
    cell: Rc<StoreCell>,
    calls: Rc<Calls>,
    /// The identity of the store, which never changes.
    id: StoreId,
    /// The engine of the store, which never changes.
    engine: Engine,
}

impl Owner {
    /// The owner of `store`.
    pub fn new(store: WebStore) -> Self {
        let calls = store.calls().clone();
        let id = store.data().id();
        let engine = store.data().engine().clone();
        let cell = Rc::new(StoreCell::new(store));
        calls.attach(Rc::downgrade(&cell));
        Self {
            cell,
            calls,
            id,
            engine,
        }
    }

    /// The store, after every flight's permit ended.
    ///
    /// [`Error::Backend`] where a host function that a flight called runs
    /// and reached the store through the owner: a host function that holds
    /// its store by a global, after the host forgot the future that waited
    /// for the flight.
    fn store(&self) -> Result<&WebStore> {
        self.calls.claim()?;
        // SAFETY: `claim` succeeded, so no host function that a flight
        // called runs, and no flight's reference to the store lives, since
        // a flight makes one only for the length of one such call. `claim`
        // also ended every flight's permit, so no flight makes a reference
        // from here on. A lease lives only inside a method of the store,
        // which borrows the owner, or inside a host function that a flight
        // called, which `claim` ruled out.
        Ok(unsafe { &*self.cell.get() })
    }

    /// The store, mutably, after every flight's permit ended, or
    /// [`Error::Backend`] where `store` is.
    fn store_mut(&mut self) -> Result<&mut WebStore> {
        self.calls.claim()?;
        // SAFETY: as in `store`, and `&mut self` makes this the owner's one
        // reference.
        Ok(unsafe { &mut *self.cell.get() })
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.calls.drop_owner();
    }
}

impl BackendStore for Owner {
    /// # Panics
    ///
    /// Where a host function that a flight called runs and reaches the
    /// store through its owner, which refuses then.
    fn data(&self) -> &StoreData {
        match self.store() {
            Ok(store) => store.data(),
            Err(error) => panic!("{error}"),
        }
    }

    /// # Panics
    ///
    /// As [`data`](BackendStore::data).
    fn data_mut(&mut self) -> &mut StoreData {
        match self.store_mut() {
            Ok(store) => store.data_mut(),
            Err(error) => panic!("{error}"),
        }
    }

    fn id(&self) -> StoreId {
        self.id
    }

    fn engine(&self) -> &Engine {
        &self.engine
    }

    fn instantiate<'a>(
        &'a mut self,
        module: &'a dyn BackendModule,
        imports: &'a [Extern],
    ) -> BoxFuture<'a, Result<Instance>> {
        // Each step reaches the store anew, and holds no reference to it
        // across the await, while the start function can run.
        Box::pin(async move {
            let (flight, object) = self.store_mut()?.start_instantiation(module, imports)?;
            let stop = flight.stop().await;
            self.store_mut()?
                .finish_instantiation(module, imports, &object, stop)
        })
    }

    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
        self.store_mut()?.instance_export(instance, name)
    }

    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
        self.store_mut()?.func_new(ty, func)
    }

    fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
        self.store()?.func_ty(func)
    }

    fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()> {
        self.store_mut()?.func_call(func, params, results)
    }

    fn func_call_resumable<'a>(
        &'a mut self,
        func: Func,
        params: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(async move {
            let (flight, returns) =
                self.store_mut()?
                    .start_resumable(func, params, results.len())?;
            let stop = flight.stop().await;
            self.store_mut()?
                .finish_resumable(flight, returns, stop, results)
        })
    }

    fn resume_call<'a>(
        &'a mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(async move {
            let call = call
                .into_any()
                .downcast::<WebSuspendedCall>()
                .map_err(|_| Error::WrongStore)?;
            let WebSuspendedCall { flight, returns } = *call;
            self.store_mut()?.resume_flight(&flight, import_results)?;
            // The resumed stack runs on a microtask, and reaches the store
            // as the flight, whose permit is the epoch from here until the
            // host next reaches the store.
            let stop = flight.stop().await;
            self.store_mut()?
                .finish_resumable(flight, returns, stop, results)
        })
    }

    fn func_start_resumable(&mut self, func: Func, params: &[Val]) -> Result<Resumption> {
        self.store_mut()?.func_start_resumable(func, params)
    }

    fn start_resume(
        &mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &[Val],
    ) -> Result<Resumption> {
        let call = call
            .into_any()
            .downcast::<WebSuspendedCall>()
            .map_err(|_| Error::WrongStore)?;
        let WebSuspendedCall { flight, returns } = *call;
        let store = self.store_mut()?;
        store.resume_flight(&flight, import_results)?;
        Ok(Resumption::new(
            store.data().id(),
            Box::new(WebResumption { flight, returns }),
        ))
    }

    fn stop_resumption<'a>(
        &'a mut self,
        resumption: &'a mut dyn BackendResumption,
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        Box::pin(async move {
            let resumption = resumption
                .as_any_mut()
                .downcast_mut::<WebResumption>()
                .ok_or(Error::WrongStore)?;
            let flight = resumption.flight.clone();
            // The wait takes the flight up: its permit is the epoch from
            // here until the host next reaches the store, and a stack that
            // parked while nothing waited for it runs on.
            self.store_mut()?.adopt(&flight)?;
            let stop = flight.stop().await;
            self.store_mut()?
                .finish_resumable(flight, resumption.returns.clone(), stop, results)
        })
    }

    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
        self.store_mut()?.memory_new(ty)
    }

    fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
        self.store()?.memory_ty(memory)
    }

    fn memory_size(&self, memory: Memory) -> Result<u64> {
        self.store()?.memory_size(memory)
    }

    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
        self.store_mut()?.memory_grow(memory, pages)
    }

    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
        self.store()?.memory_read(memory, offset, buffer)
    }

    fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()> {
        self.store_mut()?.memory_write(memory, offset, bytes)
    }

    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        self.store()?.memory_with_bytes(memory, offset, len, f)
    }

    fn memory_copy(
        &mut self,
        source: Memory,
        source_offset: u64,
        destination: Memory,
        destination_offset: u64,
        len: u64,
    ) -> Result<()> {
        self.store_mut()?
            .memory_copy(source, source_offset, destination, destination_offset, len)
    }

    fn memory_load_u8(&self, memory: Memory, offset: u64) -> Result<u8> {
        self.store()?.memory_load_u8(memory, offset)
    }

    fn memory_load_u16(&self, memory: Memory, offset: u64) -> Result<u16> {
        self.store()?.memory_load_u16(memory, offset)
    }

    fn memory_load_u32(&self, memory: Memory, offset: u64) -> Result<u32> {
        self.store()?.memory_load_u32(memory, offset)
    }

    fn memory_load_u64(&self, memory: Memory, offset: u64) -> Result<u64> {
        self.store()?.memory_load_u64(memory, offset)
    }

    fn memory_store_u8(&mut self, memory: Memory, offset: u64, value: u8) -> Result<()> {
        self.store_mut()?.memory_store_u8(memory, offset, value)
    }

    fn memory_store_u16(&mut self, memory: Memory, offset: u64, value: u16) -> Result<()> {
        self.store_mut()?.memory_store_u16(memory, offset, value)
    }

    fn memory_store_u32(&mut self, memory: Memory, offset: u64, value: u32) -> Result<()> {
        self.store_mut()?.memory_store_u32(memory, offset, value)
    }

    fn memory_store_u64(&mut self, memory: Memory, offset: u64, value: u64) -> Result<()> {
        self.store_mut()?.memory_store_u64(memory, offset, value)
    }

    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
        self.store_mut()?.global_new(ty, value)
    }

    fn global_ty(&self, global: Global) -> Result<GlobalType> {
        self.store()?.global_ty(global)
    }

    fn global_get(&mut self, global: Global) -> Result<Val> {
        self.store_mut()?.global_get(global)
    }

    fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
        self.store_mut()?.global_set(global, value)
    }

    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
        self.store_mut()?.table_new(ty, init)
    }

    fn table_ty(&self, table: Table) -> Result<TableType> {
        self.store()?.table_ty(table)
    }

    fn table_size(&self, table: Table) -> Result<u64> {
        self.store()?.table_size(table)
    }

    fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
        self.store_mut()?.table_get(table, index)
    }

    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
        self.store_mut()?.table_set(table, index, value)
    }

    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
        self.store_mut()?.table_grow(table, delta, init)
    }

    fn tag_ty(&self, tag: Tag) -> Result<TagType> {
        self.store()?.tag_ty(tag)
    }

    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
        self.store_mut()?.extern_ref_new(value)
    }

    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
        self.store()?.extern_ref_data(extern_ref)
    }

    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        self.store_mut()?.any_ref_from_i31(value)
    }

    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        self.store()?.any_ref_as_i31(any_ref)
    }
}
