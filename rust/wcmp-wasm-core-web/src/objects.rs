//! Every object a handle of a store names.

use core::any::Any;
use std::collections::HashMap;

use js_sys::{Function, Map, Object, WebAssembly};
use wasm_bindgen::JsValue;
use wcmp_wasm_core::backend::{RawHandle, StoreId};
use wcmp_wasm_core::{
    AnyRef, Error, ExportType, Extern, ExternRef, Func, FuncType, Global, GlobalType, Instance,
    Memory, MemoryType, Result, Table, TableType, Tag, TagType,
};

/// An instance: the object of its exports, the description of each export,
/// and the handle of each export the host asked for.
pub struct InstanceObject {
    pub exports: Object,
    pub types: Vec<ExportType>,
    pub handles: HashMap<String, Extern>,
}

/// A function, and its type where the backend knows it.
///
/// The backend knows the type of each export, from the boundary of its
/// module. The JavaScript API does not tell the type of a function
/// reference that a guest hands out.
pub struct FuncObject {
    pub function: Function,
    pub ty: Option<FuncType>,
}

/// A memory and its type.
pub struct MemoryObject {
    pub memory: WebAssembly::Memory,
    pub ty: MemoryType,
}

/// A global and its type.
pub struct GlobalObject {
    pub global: WebAssembly::Global,
    pub ty: GlobalType,
}

/// A table and its type.
pub struct TableObject {
    pub table: WebAssembly::Table,
    pub ty: TableType,
}

/// A tag and its type.
pub struct TagObject {
    pub tag: JsValue,
    pub ty: TagType,
}

/// An `externref`: the JavaScript value a guest holds, and the host's value
/// where the host made the reference.
///
/// The value of a reference the host made is a fresh, empty JavaScript
/// object, so the backend knows it again by its identity when a guest hands
/// it back. A reference the host did not make, such as an internal
/// reference a guest converted, has no host value.
pub struct ExternRefObject {
    pub value: JsValue,
    pub data: Option<Box<dyn Any + Send + Sync>>,
}

/// Every object a handle of one store names.
///
/// A handle is the index of its object in the list of its kind. A handle
/// is `Copy` and the host never releases one, so the lists only grow, and
/// each object stays for the life of the store. Each entry holds its
/// JavaScript value, which roots it: the browser's collector cannot free a
/// value that an entry holds. When the store drops, every entry drops, and
/// the collector can free each value that nothing else holds.
pub struct Objects {
    id: StoreId,
    instances: Vec<InstanceObject>,
    funcs: Vec<FuncObject>,
    memories: Vec<MemoryObject>,
    globals: Vec<GlobalObject>,
    tables: Vec<TableObject>,
    tags: Vec<TagObject>,
    extern_refs: Vec<ExternRefObject>,
    any_refs: Vec<JsValue>,
    /// The index of each function in `funcs`, by the function itself, so a
    /// function that crosses again keeps its handle and its type.
    func_indices: Map,
    /// The index of each `externref` in `extern_refs`, by its value.
    extern_ref_indices: Map,
}

impl Objects {
    /// The objects of the store `id`, none yet.
    pub fn new(id: StoreId) -> Self {
        Self {
            id,
            instances: Vec::new(),
            funcs: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            tables: Vec::new(),
            tags: Vec::new(),
            extern_refs: Vec::new(),
            any_refs: Vec::new(),
            func_indices: Map::new(),
            extern_ref_indices: Map::new(),
        }
    }

    /// The handle of `function`: the handle it already has in the store,
    /// or a new one of type `ty`.
    ///
    /// A function that already has a handle keeps it, and gains `ty` where
    /// its type was not known.
    pub fn add_func(&mut self, function: Function, ty: Option<FuncType>) -> Func {
        if let Some(index) = known(&self.func_indices, &function) {
            if let Some(object) = self.funcs.get_mut(index as usize)
                && object.ty.is_none()
            {
                object.ty = ty;
            }
            return Func::from_raw(self.id, index);
        }
        let index = self.funcs.len() as u64;
        self.func_indices
            .set(&function, &JsValue::from_f64(index as f64));
        self.funcs.push(FuncObject { function, ty });
        Func::from_raw(self.id, index)
    }

    /// The handle of the `externref` whose value is `value`: the handle of
    /// the reference the host made with that value, or a new handle with no
    /// host value.
    pub fn add_extern_ref_value(&mut self, value: JsValue) -> ExternRef {
        if let Some(index) = known(&self.extern_ref_indices, &value) {
            return ExternRef::from_raw(self.id, index);
        }
        self.push_extern_ref(ExternRefObject { value, data: None })
    }

    /// The handle of a new `externref` that holds the host's value `data`.
    pub fn add_extern_ref(&mut self, data: Box<dyn Any + Send + Sync>) -> ExternRef {
        let value = JsValue::from(Object::new());
        self.push_extern_ref(ExternRefObject {
            value,
            data: Some(data),
        })
    }

    fn push_extern_ref(&mut self, object: ExternRefObject) -> ExternRef {
        let index = self.extern_refs.len() as u64;
        self.extern_ref_indices
            .set(&object.value, &JsValue::from_f64(index as f64));
        self.extern_refs.push(object);
        ExternRef::from_raw(self.id, index)
    }
}

/// The index that `map` holds for `key`.
fn known(map: &Map, key: &JsValue) -> Option<u64> {
    map.get(key).as_f64().map(|index| index as u64)
}

/// Adds, for each kind of object, a method that finds the object of a
/// handle, and where named, a method that keeps an object and returns its
/// handle.
macro_rules! objects {
    ($($list:ident: $object:ty => $handle:ty, $get:ident $(, $add:ident)?;)*) => {
        impl Objects {
            $(
                $(
                    /// The handle of `object`, which the store keeps from
                    /// now on.
                    pub fn $add(&mut self, object: $object) -> $handle {
                        self.$list.push(object);
                        <$handle>::from_raw(self.id, (self.$list.len() - 1) as u64)
                    }
                )?

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
    instances: InstanceObject => Instance, instance, add_instance;
    funcs: FuncObject => Func, func;
    memories: MemoryObject => Memory, memory, add_memory;
    globals: GlobalObject => Global, global, add_global;
    tables: TableObject => Table, table, add_table;
    tags: TagObject => Tag, tag, add_tag;
    extern_refs: ExternRefObject => ExternRef, extern_ref;
    any_refs: JsValue => AnyRef, any_ref, add_any_ref;
}

impl Objects {
    /// The instance that `handle` names, mutably.
    pub fn instance_mut(&mut self, handle: Instance) -> Result<&mut InstanceObject> {
        usize::try_from(handle.index())
            .ok()
            .and_then(|index| self.instances.get_mut(index))
            .ok_or(Error::WrongStore)
    }
}
