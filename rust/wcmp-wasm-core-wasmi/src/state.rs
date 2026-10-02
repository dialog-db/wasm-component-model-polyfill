// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the backend keeps in each Wasmi store.

use wcmp_wasm_core::backend::{RawHandle, StoreData};
use wcmp_wasm_core::{
    Capability, Error, Extern, ExternRef, Func, Global, Instance, Memory, Result, Table,
};

/// What the backend keeps in each Wasmi store: the engine's data for the
/// store, every object a handle of the store names, and how many calls of
/// host functions run in the store, each inside the last.
///
/// A handle is the index of its object in the list of its kind. The lists
/// only grow: a handle is `Copy`, and the host never releases one, so each
/// object stays for the life of the store. Wasmi keeps every object of a
/// store, an `externref` included, as long as the store lives, so the lists
/// hold Wasmi's handles and nothing more.
pub struct State {
    data: StoreData,
    instances: Vec<wasmi::Instance>,
    funcs: Vec<wasmi::Func>,
    memories: Vec<wasmi::Memory>,
    globals: Vec<wasmi::Global>,
    tables: Vec<wasmi::Table>,
    extern_refs: Vec<wasmi::ExternRef>,
    host_depth: u32,
}

impl State {
    /// The state of a new store whose engine data is `data`.
    pub fn new(data: StoreData) -> Self {
        Self {
            data,
            instances: Vec::new(),
            funcs: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            tables: Vec::new(),
            extern_refs: Vec::new(),
            host_depth: 0,
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

    /// How many calls of host functions run in the store now, each inside
    /// the last.
    pub fn host_depth(&self) -> u32 {
        self.host_depth
    }

    /// Records that `depth` calls of host functions run in the store now.
    pub fn set_host_depth(&mut self, depth: u32) {
        self.host_depth = depth;
    }

    /// The handle of the Wasmi extern `external`, which the store keeps
    /// from now on.
    pub fn add_extern(&mut self, external: wasmi::Extern) -> Extern {
        match external {
            wasmi::Extern::Func(func) => Extern::Func(self.add_func(func)),
            wasmi::Extern::Global(global) => Extern::Global(self.add_global(global)),
            wasmi::Extern::Table(table) => Extern::Table(self.add_table(table)),
            wasmi::Extern::Memory(memory) => Extern::Memory(self.add_memory(memory)),
        }
    }

    /// The Wasmi extern that `external` names.
    ///
    /// Wasmi has no tags, so a tag is [`Error::Unsupported`] with
    /// `exceptions`.
    pub fn to_extern(&self, external: &Extern) -> Result<wasmi::Extern> {
        Ok(match external {
            Extern::Func(func) => wasmi::Extern::Func(*self.func(*func)?),
            Extern::Global(global) => wasmi::Extern::Global(*self.global(*global)?),
            Extern::Table(table) => wasmi::Extern::Table(*self.table(*table)?),
            Extern::Memory(memory) => wasmi::Extern::Memory(*self.memory(*memory)?),
            Extern::Tag(_) => return Err(Error::Unsupported(Capability::Exceptions)),
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
    instances: wasmi::Instance => Instance, add_instance, instance;
    funcs: wasmi::Func => Func, add_func, func;
    memories: wasmi::Memory => Memory, add_memory, memory;
    globals: wasmi::Global => Global, add_global, global;
    tables: wasmi::Table => Table, add_table, table;
    extern_refs: wasmi::ExternRef => ExternRef, add_extern_ref, extern_ref;
}
