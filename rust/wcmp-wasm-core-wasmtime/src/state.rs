//! What the backend keeps in each Wasmtime store.

use std::sync::Arc;

use wasmtime::OwnedRooted;
use wcmp_wasm_core::backend::{RawHandle, StoreData};
use wcmp_wasm_core::{
    AnyRef, Error, ExnRef, Extern, ExternRef, Func, Global, Instance, Memory, Result, Table, Tag,
};

use crate::memory_object::MemoryObject;
use crate::type_registry::TypeRegistry;

/// What the backend keeps in each Wasmtime store: the engine's data for
/// the store, and every object a handle of the store names.
///
/// A handle is the index of its object in the list of its kind. The lists
/// only grow: a handle is `Copy`, and the host never releases one, so each
/// object stays for the life of the store. A GC reference is held by an
/// `OwnedRooted`, which keeps it alive until the store drops.
pub struct State {
    data: StoreData,
    types: Arc<TypeRegistry>,
    instances: Vec<wasmtime::Instance>,
    funcs: Vec<wasmtime::Func>,
    memories: Vec<MemoryObject>,
    globals: Vec<wasmtime::Global>,
    tables: Vec<wasmtime::Table>,
    tags: Vec<wasmtime::Tag>,
    extern_refs: Vec<OwnedRooted<wasmtime::ExternRef>>,
    any_refs: Vec<OwnedRooted<wasmtime::AnyRef>>,
    exn_refs: Vec<OwnedRooted<wasmtime::ExnRef>>,
}

impl State {
    /// The state of a new store whose engine data is `data`, over the
    /// concrete types `types` of the backend.
    pub fn new(data: StoreData, types: Arc<TypeRegistry>) -> Self {
        Self {
            data,
            types,
            instances: Vec::new(),
            funcs: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            tables: Vec::new(),
            tags: Vec::new(),
            extern_refs: Vec::new(),
            any_refs: Vec::new(),
            exn_refs: Vec::new(),
        }
    }

    /// The engine's data for the store.
    pub fn data(&self) -> &StoreData {
        &self.data
    }

    /// The engine's data for the store, mutably.
    pub fn data_mut(&mut self) -> &mut StoreData {
        &mut self.data
    }

    /// The concrete types of the backend.
    pub fn types(&self) -> &Arc<TypeRegistry> {
        &self.types
    }

    /// The handle of the Wasmtime extern `external`, which the store keeps
    /// from now on.
    pub fn add_extern(&mut self, external: wasmtime::Extern) -> Extern {
        match external {
            wasmtime::Extern::Func(func) => Extern::Func(self.add_func(func)),
            wasmtime::Extern::Global(global) => Extern::Global(self.add_global(global)),
            wasmtime::Extern::Table(table) => Extern::Table(self.add_table(table)),
            wasmtime::Extern::Memory(memory) => {
                Extern::Memory(self.add_memory(MemoryObject::Unshared(memory)))
            }
            wasmtime::Extern::SharedMemory(memory) => {
                Extern::Memory(self.add_memory(MemoryObject::Shared(memory)))
            }
            wasmtime::Extern::Tag(tag) => Extern::Tag(self.add_tag(tag)),
        }
    }

    /// The Wasmtime extern that `external` names.
    pub fn to_extern(&self, external: &Extern) -> Result<wasmtime::Extern> {
        Ok(match external {
            Extern::Func(func) => wasmtime::Extern::Func(*self.func(*func)?),
            Extern::Global(global) => wasmtime::Extern::Global(*self.global(*global)?),
            Extern::Table(table) => wasmtime::Extern::Table(*self.table(*table)?),
            Extern::Memory(memory) => self.memory(*memory)?.to_extern(),
            Extern::Tag(tag) => wasmtime::Extern::Tag(*self.tag(*tag)?),
        })
    }
}

/// Adds, for each kind of object, a method that keeps an object and
/// returns its handle, and a method that finds the object of a handle.
macro_rules! objects {
    ($($list:ident: $object:ty => $handle:ty, $add:ident, $get:ident;)*) => {
        impl State {
            $(
                /// The handle of `object`, which the store keeps from now on.
                pub fn $add(&mut self, object: $object) -> $handle {
                    self.$list.push(object);
                    <$handle>::from_raw(self.data.id(), (self.$list.len() - 1) as u64)
                }

                /// The object that `handle` names.
                ///
                /// The engine already checked that the handle names this
                /// store, so an index the store does not know is a handle
                /// the host forged: [`Error::WrongStore`].
                pub fn $get(&self, handle: $handle) -> Result<&$object> {
                    usize::try_from(handle.index())
                        .ok()
                        .and_then(|index| self.$list.get(index))
                        .ok_or(Error::WrongStore)
                }
            )*
        }
    };
}

objects! {
    instances: wasmtime::Instance => Instance, add_instance, instance;
    funcs: wasmtime::Func => Func, add_func, func;
    memories: MemoryObject => Memory, add_memory, memory;
    globals: wasmtime::Global => Global, add_global, global;
    tables: wasmtime::Table => Table, add_table, table;
    tags: wasmtime::Tag => Tag, add_tag, tag;
    extern_refs: OwnedRooted<wasmtime::ExternRef> => ExternRef, add_extern_ref, extern_ref;
    any_refs: OwnedRooted<wasmtime::AnyRef> => AnyRef, add_any_ref, any_ref;
    exn_refs: OwnedRooted<wasmtime::ExnRef> => ExnRef, add_exn_ref, exn_ref;
}
