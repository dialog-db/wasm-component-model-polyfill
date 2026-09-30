//! A store as a backend holds it.

use core::any::Any;

use crate::call::{ResumableCall, Resumption};
use crate::capability::Capability;
use crate::contract::{
    BackendModule, BackendResumption, BackendSuspendedCall, BoxFuture, HostFunc, MaybeSend,
};
use crate::error::{Error, Result};
use crate::externs::{Extern, Func, Global, Instance, Memory, Table, Tag};
use crate::store::StoreData;
use crate::types::{FuncType, GlobalType, MemoryType, TableType, TagType};
use crate::values::{AnyRef, ExternRef, I31, Val};

/// A store, as the backend that made it holds it.
///
/// Every operation on an instance, or on an object that a store owns, is a
/// method of this trait. The engine checks that each handle it passes
/// belongs to this store. The backend still validates the index inside each
/// handle, and returns [`Error::WrongStore`] for one it does not know.
///
/// The same trait serves a store while a guest runs in it: a host function
/// receives the store as `&mut dyn BackendStore`, and can call back into the
/// guest through it, at any depth.
///
/// Each memory method checks its range against the size of the memory. A
/// range outside the memory is [`Error::MemoryOutOfBounds`], never a panic
/// and never an abort. The scalar loads and stores are little-endian, as
/// WebAssembly is. They have default bodies over [`memory_read`] and
/// [`memory_write`], which a backend can replace with faster ones.
///
/// A method that needs a capability has a default body that returns
/// [`Error::Unsupported`] with the name of the capability. A backend that
/// declares the capability replaces it.
///
/// [`memory_read`]: BackendStore::memory_read
/// [`memory_write`]: BackendStore::memory_write
pub trait BackendStore: MaybeSend {
    /// The data the store was made with.
    fn data(&self) -> &StoreData;

    /// The data the store was made with, mutably.
    fn data_mut(&mut self) -> &mut StoreData;

    /// Instantiates `module` with `imports`, one extern for each import of
    /// the module, in order.
    ///
    /// `module` was compiled by the backend of this store. An import of the
    /// wrong kind or type is [`Error::Link`], and a trap in the start
    /// function of the module is [`Error::Trap`].
    fn instantiate<'a>(
        &'a mut self,
        module: &'a dyn BackendModule,
        imports: &'a [Extern],
    ) -> BoxFuture<'a, Result<Instance>>;

    /// The export of `instance` named `name`, or `None` where the instance
    /// exports nothing by that name.
    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>>;

    /// Makes a host function of type `ty`.
    ///
    /// A suspending host function ([`HostFunc::is_suspending`]) reaches the
    /// backend only where it declares
    /// [`host_suspension`](Capability::HostSuspension).
    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func>;

    /// The type of `func`, or `None` where the engine does not know it.
    fn func_ty(&self, func: Func) -> Result<Option<FuncType>>;

    /// Calls `func` with `params`, and writes its results to `results`.
    fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()>;

    /// Calls `func` as a resumable call.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`host_suspension`](Capability::HostSuspension).
    fn func_call_resumable<'a>(
        &'a mut self,
        func: Func,
        params: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        let _ = (func, params, results);
        Box::pin(core::future::ready(Err(Error::Unsupported(
            Capability::HostSuspension,
        ))))
    }

    /// Resumes `call`, a call of this store that waits, with
    /// `import_results`, the results of the suspending host function, and
    /// runs it to its next suspension or its end. At its end, the results of
    /// the call are in `results`.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`host_suspension`](Capability::HostSuspension), and only with a call
    /// that this store made. The backend downcasts `call` through
    /// [`BackendSuspendedCall::into_any`] to its own type. A call of a type
    /// the backend does not know is [`Error::WrongStore`].
    ///
    /// The store resumes the call, and not the call itself, so the backend
    /// reaches its own concrete store here. Where a host function resumes a
    /// call, this store is the context the host function received.
    fn resume_call<'a>(
        &'a mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &'a [Val],
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        let _ = (call, import_results, results);
        Box::pin(core::future::ready(Err(Error::Unsupported(
            Capability::HostSuspension,
        ))))
    }

    /// Starts `func` as a resumable call, runs its first stretch, and
    /// answers the call as a resumption that
    /// [`stop_resumption`](BackendStore::stop_resumption) waits for.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`host_suspension`](Capability::HostSuspension).
    fn func_start_resumable(&mut self, func: Func, params: &[Val]) -> Result<Resumption> {
        let _ = (func, params);
        Err(Error::Unsupported(Capability::HostSuspension))
    }

    /// Resumes `call`, a call of this store that waits, with
    /// `import_results`, the results of the suspending host function, and
    /// answers the call as a resumption that
    /// [`stop_resumption`](BackendStore::stop_resumption) waits for.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`host_suspension`](Capability::HostSuspension), and only with a call
    /// that this store made, as for
    /// [`resume_call`](BackendStore::resume_call).
    fn start_resume(
        &mut self,
        call: Box<dyn BackendSuspendedCall>,
        import_results: &[Val],
    ) -> Result<Resumption> {
        let _ = (call, import_results);
        Err(Error::Unsupported(Capability::HostSuspension))
    }

    /// Waits for the next stop of `resumption`, a call of this store that
    /// runs, and answers how it ended. Where it finished, its results are in
    /// `results`.
    ///
    /// Where the future drops before the call stops, the host has the store
    /// back, and the call keeps its place: a backend that runs the call on a
    /// microtask lets it run on until it would next reach the store, and it
    /// waits there. A later wait grants it the store again and takes it up
    /// from where it waits. So any number of waits may drop before one sees
    /// the stop.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`host_suspension`](Capability::HostSuspension), only with a
    /// resumption that this store made, and never again once a wait saw its
    /// stop.
    fn stop_resumption<'a>(
        &'a mut self,
        resumption: &'a mut dyn BackendResumption,
        results: &'a mut [Val],
    ) -> BoxFuture<'a, Result<ResumableCall>> {
        let _ = (resumption, results);
        Box::pin(core::future::ready(Err(Error::Unsupported(
            Capability::HostSuspension,
        ))))
    }

    /// Makes a memory of type `ty`.
    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory>;

    /// The type of `memory`.
    fn memory_ty(&self, memory: Memory) -> Result<MemoryType>;

    /// The size of `memory`, in bytes.
    fn memory_size(&self, memory: Memory) -> Result<u64>;

    /// Grows `memory` by `pages` pages, and returns its old size in pages.
    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64>;

    /// Copies the bytes of `memory` at `offset` into `buffer`.
    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()>;

    /// Copies `bytes` into `memory` at `offset`.
    fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()>;

    /// Lends the `len` bytes of `memory` at `offset` to `f`, and calls `f`
    /// exactly once.
    ///
    /// Where the backend can, it lends the bytes of the memory itself. It
    /// never lends a shared memory: it copies the range with atomic reads,
    /// and lends the copy.
    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()>;

    /// Copies `len` bytes from `source` at `source_offset` to `destination`
    /// at `destination_offset`, with no buffer on the host. The two can be
    /// one memory, and the ranges can overlap.
    fn memory_copy(
        &mut self,
        source: Memory,
        source_offset: u64,
        destination: Memory,
        destination_offset: u64,
        len: u64,
    ) -> Result<()>;

    /// Reads the byte of `memory` at `offset`.
    fn memory_load_u8(&self, memory: Memory, offset: u64) -> Result<u8> {
        let mut bytes = [0; 1];
        self.memory_read(memory, offset, &mut bytes)?;
        Ok(u8::from_le_bytes(bytes))
    }

    /// Reads the little-endian `u16` of `memory` at `offset`.
    fn memory_load_u16(&self, memory: Memory, offset: u64) -> Result<u16> {
        let mut bytes = [0; 2];
        self.memory_read(memory, offset, &mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }

    /// Reads the little-endian `u32` of `memory` at `offset`.
    fn memory_load_u32(&self, memory: Memory, offset: u64) -> Result<u32> {
        let mut bytes = [0; 4];
        self.memory_read(memory, offset, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// Reads the little-endian `u64` of `memory` at `offset`.
    fn memory_load_u64(&self, memory: Memory, offset: u64) -> Result<u64> {
        let mut bytes = [0; 8];
        self.memory_read(memory, offset, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    /// Writes the byte `value` to `memory` at `offset`.
    fn memory_store_u8(&mut self, memory: Memory, offset: u64, value: u8) -> Result<()> {
        self.memory_write(memory, offset, &value.to_le_bytes())
    }

    /// Writes `value` to `memory` at `offset`, little-endian.
    fn memory_store_u16(&mut self, memory: Memory, offset: u64, value: u16) -> Result<()> {
        self.memory_write(memory, offset, &value.to_le_bytes())
    }

    /// Writes `value` to `memory` at `offset`, little-endian.
    fn memory_store_u32(&mut self, memory: Memory, offset: u64, value: u32) -> Result<()> {
        self.memory_write(memory, offset, &value.to_le_bytes())
    }

    /// Writes `value` to `memory` at `offset`, little-endian.
    fn memory_store_u64(&mut self, memory: Memory, offset: u64, value: u64) -> Result<()> {
        self.memory_write(memory, offset, &value.to_le_bytes())
    }

    /// Makes a global of type `ty` that holds `value`.
    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global>;

    /// The type of `global`.
    fn global_ty(&self, global: Global) -> Result<GlobalType>;

    /// The value `global` holds.
    fn global_get(&mut self, global: Global) -> Result<Val>;

    /// Sets the value of `global`, which is mutable.
    fn global_set(&mut self, global: Global, value: Val) -> Result<()>;

    /// Makes a table of type `ty` whose every element is `init`.
    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table>;

    /// The type of `table`.
    fn table_ty(&self, table: Table) -> Result<TableType>;

    /// The number of elements of `table`.
    fn table_size(&self, table: Table) -> Result<u64>;

    /// The element of `table` at `index`. An index outside the table is
    /// [`Error::TableOutOfBounds`].
    fn table_get(&mut self, table: Table, index: u64) -> Result<Val>;

    /// Sets the element of `table` at `index`. An index outside the table is
    /// [`Error::TableOutOfBounds`].
    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()>;

    /// Grows `table` by `delta` elements that are each `init`, and returns
    /// its old size.
    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64>;

    /// The type of `tag`.
    fn tag_ty(&self, tag: Tag) -> Result<TagType>;

    /// Makes an `externref` that holds `value`.
    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef>;

    /// The value that `extern_ref` holds.
    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)>;

    /// Makes the `i31ref` of `value`.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`gc`](Capability::Gc).
    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        let _ = value;
        Err(Error::Unsupported(Capability::Gc))
    }

    /// The integer of `any_ref`, or `None` where it is not an `i31ref`.
    ///
    /// The engine reaches this method only where the backend declares
    /// [`gc`](Capability::Gc).
    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        let _ = any_ref;
        Err(Error::Unsupported(Capability::Gc))
    }
}
