//! A store of the browser backend.

use core::any::Any;
use core::task::Poll;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen::{JsCast, JsValue};
use wcmp_wasm_core::backend::{
    BackendModule, BackendStore, BoxFuture, HostFunc, RawHandle, StoreData,
};
use wcmp_wasm_core::{
    AnyRef, Capability, Error, Extern, ExternRef, ExternType, Func, FuncType, Global, GlobalType,
    HeapType, I31, ImportType, Instance, Memory, MemoryType, Mutability, Result, Table, TableType,
    Tag, TagType, Val, ValType,
};

use crate::calls::Calls;
use crate::carrier::Carrier;
use crate::errors;
use crate::js;
use crate::module::WebModule;
use crate::objects::{GlobalObject, InstanceObject, MemoryObject, Objects, TableObject, TagObject};
use crate::type_registry::TypeRegistry;
use crate::values::{self, Kind};
use crate::wrapper::{self, Wrappers};

/// A store of the browser backend: the engine's data for the store, and
/// every object a handle of the store names.
///
/// The browser owns each instance and each object an instance makes. The
/// store holds the JavaScript value of each, which keeps it alive for the
/// life of the store.
pub struct WebStore {
    data: StoreData,
    types: Rc<TypeRegistry>,
    objects: Objects,
    carrier: Carrier,
    calls: Rc<Calls>,
    wrappers: Wrappers,
}

impl WebStore {
    /// The store whose engine data is `data`, over the concrete types
    /// `types` of the backend.
    pub fn new(data: StoreData, types: Rc<TypeRegistry>) -> Self {
        let objects = Objects::new(data.id());
        let carrier = Carrier::new(data.id());
        let calls = Rc::new(Calls::new());
        let wrappers = Wrappers::new(calls.clone());
        Self {
            data,
            types,
            objects,
            carrier,
            calls,
            wrappers,
        }
    }

    /// Runs the host function `func` of type `ty` in this store, a guest's
    /// store, with the arguments `args` that its wrapper carried, and
    /// returns its results as the wrapper carries them.
    ///
    /// The host function receives this store, and can call back into a
    /// guest through it, at any depth.
    pub fn call_host(
        &mut self,
        ty: &FuncType,
        func: &HostFunc,
        args: Vec<JsValue>,
    ) -> anyhow::Result<Vec<JsValue>> {
        let params = wrapper::params(&mut self.objects, &self.types, self.data.id(), ty, args)?;
        let mut results = ty
            .results()
            .iter()
            .map(|ty| Val::default_for_ty(ty).unwrap_or(Val::I32(0)))
            .collect::<Vec<_>>();
        match func.call(self, &params, &mut results)? {
            Poll::Ready(()) => {}
            Poll::Pending => anyhow::bail!(
                "a host function that is not suspending answered \"not yet\" to a guest"
            ),
        }
        Ok(wrapper::results(
            &self.objects,
            &mut self.carrier,
            &self.types,
            ty,
            &results,
        )?)
    }

    /// Calls the guest function `function` with `args`, where the host
    /// functions of this store reach it.
    ///
    /// A host function that fails inside the call traps the guest, and the
    /// call fails with [`TrapKind::Host`](wcmp_wasm_core::TrapKind::Host)
    /// and the host function's own error.
    fn apply(&mut self, function: &Function, args: &Array) -> Result<JsValue> {
        let calls = self.calls.clone();
        let entry = calls.enter(self);
        let returned = Reflect::apply(function, &JsValue::UNDEFINED, args);
        drop(entry);
        returned.map_err(|error| calls.trap().unwrap_or_else(|| errors::call(&error)))
    }

    /// The imports object of an instantiation of a module whose imports are
    /// `types`, with the externs `imports`, in order.
    ///
    /// The object holds each extern's own JavaScript value, so an exported
    /// function reaches the import of another instance as the function
    /// object of the export itself, and a call between two instances is a
    /// call from WebAssembly to WebAssembly.
    ///
    /// The JavaScript API links one value to each pair of names. Where a
    /// module imports one pair of names twice and the instantiation gives
    /// two different externs for them, the instantiation is
    /// [`Error::Link`].
    fn imports_object(&self, types: &[ImportType], imports: &[Extern]) -> Result<Object> {
        let object = Object::create(JsValue::NULL.unchecked_ref());
        let mut given: HashMap<(&str, &str), JsValue> = HashMap::new();
        for (ty, import) in types.iter().zip(imports) {
            let link = |message: String| Error::Link {
                module: ty.module().to_string(),
                name: ty.name().to_string(),
                message,
            };
            let expected = extern_kind(ty.ty());
            let found = kind_of_extern(import);
            if expected != found {
                return Err(link(format!("expected {expected}, found {found}")));
            }
            let value = self.extern_value(import)?;
            if let Some(earlier) = given.get(&(ty.module(), ty.name())) {
                if !Object::is(earlier, &value) {
                    return Err(link(
                        "the JavaScript API links one value to each pair of names, and the \
                         instantiation gave two different externs for this pair"
                            .to_string(),
                    ));
                }
                continue;
            }
            let namespace = match js::get(&object, ty.module()).map_err(|e| errors::call(&e))? {
                namespace if namespace.is_undefined() => {
                    let namespace: Object = Object::create(JsValue::NULL.unchecked_ref());
                    js::set(&object, ty.module(), &namespace).map_err(|e| errors::call(&e))?;
                    namespace.into()
                }
                namespace => namespace,
            };
            js::set(&namespace, ty.name(), &value).map_err(|e| errors::call(&e))?;
            given.insert((ty.module(), ty.name()), value);
        }
        Ok(object)
    }

    /// The JavaScript value of `external`.
    fn extern_value(&self, external: &Extern) -> Result<JsValue> {
        Ok(match external {
            Extern::Func(func) => self.objects.func(*func)?.function.clone().into(),
            Extern::Global(global) => self.objects.global(*global)?.global.clone().into(),
            Extern::Table(table) => self.objects.table(*table)?.table.clone().into(),
            Extern::Memory(memory) => self.objects.memory(*memory)?.memory.clone().into(),
            Extern::Tag(tag) => self.objects.tag(*tag)?.tag.clone(),
        })
    }

    /// The handle of the export `value` of type `ty`, named `name`.
    fn add_export(&mut self, name: &str, ty: &ExternType, value: JsValue) -> Result<Extern> {
        let wrong =
            |_| errors::backend(format!("the export `{name}` is not a {}", extern_kind(ty)));
        Ok(match ty {
            ExternType::Func(ty) => {
                let function = value.dyn_into::<Function>().map_err(wrong)?;
                Extern::Func(self.objects.add_func(function, Some(ty.clone())))
            }
            ExternType::Global(ty) => Extern::Global(self.objects.add_global(GlobalObject {
                global: value.dyn_into().map_err(wrong)?,
                ty: *ty,
            })),
            ExternType::Table(ty) => Extern::Table(self.objects.add_table(TableObject {
                table: value.dyn_into().map_err(wrong)?,
                ty: *ty,
            })),
            ExternType::Memory(ty) => Extern::Memory(self.objects.add_memory(MemoryObject {
                memory: value.dyn_into().map_err(wrong)?,
                ty: *ty,
            })),
            ExternType::Tag(ty) => Extern::Tag(self.objects.add_tag(TagObject {
                tag: value,
                ty: ty.clone(),
            })),
        })
    }

    /// The kind of the values of `ty`.
    fn kind(&self, ty: &ValType) -> Kind {
        Kind::of_type(ty, &self.types)
    }

    /// The bytes of `memory` as a view over its buffer, after a check that
    /// the `len` bytes at `offset` lie inside it.
    fn memory_view(&self, memory: Memory, offset: u64, len: u64) -> Result<Uint8Array> {
        let object = self.objects.memory(memory)?;
        let buffer = object.memory.buffer();
        let size = js::get(&buffer, "byteLength")
            .ok()
            .as_ref()
            .and_then(js::count)
            .unwrap_or(0);
        let outside = || Error::MemoryOutOfBounds { offset, len, size };
        let end = offset.checked_add(len).ok_or_else(outside)?;
        if end > size {
            return Err(outside());
        }
        // A typed array addresses its buffer with 32-bit numbers here.
        let (Ok(offset), Ok(len)) = (u32::try_from(offset), u32::try_from(len)) else {
            return Err(errors::backend(
                "the browser backend reaches only the first 4 GiB of a memory",
            ));
        };
        Ok(Uint8Array::new_with_byte_offset_and_length(
            &buffer, offset, len,
        ))
    }
}

impl BackendStore for WebStore {
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
                .downcast_ref::<WebModule>()
                .ok_or(Error::WrongEngine)?;
            let object = self.imports_object(module.imports(), imports)?;
            // `WebAssembly.instantiate`, and never `new WebAssembly.Instance`,
            // which the browser refuses for a module above its limit. The
            // start function of the module can call a host function, so the
            // host functions reach the store until the instantiation ends.
            let calls = self.calls.clone();
            let entry = calls.enter(self);
            let instantiated = WebAssembly::instantiate_module(module.module(), &object).await;
            drop(entry);
            let instance = instantiated
                .map_err(|error| {
                    calls
                        .trap()
                        .unwrap_or_else(|| errors::instantiate(&error, module.imports()))
                })?
                .dyn_into::<WebAssembly::Instance>()
                .map_err(|_| errors::backend("the instantiation gave no instance"))?;
            Ok(self.objects.add_instance(InstanceObject {
                exports: instance.exports(),
                types: module.exports().to_vec(),
                handles: HashMap::new(),
            }))
        })
    }

    fn instance_export(&mut self, instance: Instance, name: &str) -> Result<Option<Extern>> {
        let object = self.objects.instance(instance)?;
        if let Some(handle) = object.handles.get(name) {
            return Ok(Some(*handle));
        }
        let Some(export) = object.types.iter().find(|export| export.name() == name) else {
            return Ok(None);
        };
        let ty = export.ty().clone();
        let value = js::get(&object.exports, name).map_err(|error| errors::call(&error))?;
        let handle = self.add_export(name, &ty, value)?;
        self.objects
            .instance_mut(instance)?
            .handles
            .insert(name.to_string(), handle);
        Ok(Some(handle))
    }

    fn func_new(&mut self, ty: FuncType, func: HostFunc) -> Result<Func> {
        if func.is_suspending() {
            return Err(Error::Unsupported(Capability::HostSuspension));
        }
        let function = self.wrappers.make(&ty, func, &mut self.carrier)?;
        Ok(self.objects.add_func(function, Some(ty)))
    }

    fn func_ty(&self, func: Func) -> Result<Option<FuncType>> {
        Ok(self.objects.func(func)?.ty.clone())
    }

    fn func_call(&mut self, func: Func, params: &[Val], results: &mut [Val]) -> Result<()> {
        let object = self.objects.func(func)?;
        let function = object.function.clone();
        let ty = object.ty.clone();
        let kinds = match &ty {
            Some(ty) => {
                if params.len() != ty.params().len() || results.len() != ty.results().len() {
                    return Err(values::mismatch(format!(
                        "the function takes {} parameters and gives {} results, and the call \
                         gave {} parameters and {} result slots",
                        ty.params().len(),
                        ty.results().len(),
                        params.len(),
                        results.len()
                    )));
                }
                for (value, ty) in params.iter().zip(ty.params()) {
                    values::check(value, ty, &self.types)?;
                }
                if Carrier::needed(ty) {
                    let (carrier, args) = self.carrier.arguments(
                        &self.objects,
                        (func.index(), &function),
                        ty,
                        params,
                    )?;
                    let returned = self.apply(&carrier, &args)?;
                    return self.carrier.results(
                        &mut self.objects,
                        &self.types,
                        ty,
                        returned,
                        results,
                    );
                }
                ty.results().iter().map(|ty| self.kind(ty)).collect()
            }
            // The JavaScript API does not tell the type of a function
            // reference a guest handed out, so the slots tell the kinds of
            // the results.
            None => results.iter().map(Kind::of_val).collect::<Vec<_>>(),
        };
        let args = params
            .iter()
            .map(|value| values::to_js(&self.objects, value))
            .collect::<Result<Array>>()?;
        let returned = self.apply(&function, &args)?;
        match results {
            [] => {}
            [result] => *result = values::from_js(&mut self.objects, returned, kinds[0])?,
            results => {
                let returned = returned
                    .dyn_into::<Array>()
                    .map_err(|_| values::mismatch("the call gave one result".to_string()))?;
                if returned.length() as usize != results.len() {
                    return Err(values::mismatch(format!(
                        "the call gave {} results for {} slots",
                        returned.length(),
                        results.len()
                    )));
                }
                for (index, (slot, kind)) in results.iter_mut().zip(kinds).enumerate() {
                    *slot = values::from_js(&mut self.objects, returned.get(index as u32), kind)?;
                }
            }
        }
        Ok(())
    }

    fn memory_new(&mut self, ty: MemoryType) -> Result<Memory> {
        let is_64 = ty.is_64();
        let mut descriptor = vec![("initial", js::address(ty.minimum(), is_64))];
        if let Some(maximum) = ty.maximum() {
            descriptor.push(("maximum", js::address(maximum, is_64)));
        }
        if ty.is_shared() {
            descriptor.push(("shared", JsValue::TRUE));
        }
        if is_64 {
            descriptor.push(("address", JsValue::from_str("i64")));
        }
        let descriptor = js::object(&descriptor).map_err(|error| errors::call(&error))?;
        let memory = WebAssembly::Memory::new(&descriptor).map_err(|error| errors::call(&error))?;
        Ok(self.objects.add_memory(MemoryObject { memory, ty }))
    }

    fn memory_ty(&self, memory: Memory) -> Result<MemoryType> {
        Ok(self.objects.memory(memory)?.ty)
    }

    fn memory_size(&self, memory: Memory) -> Result<u64> {
        let buffer = self.objects.memory(memory)?.memory.buffer();
        js::get(&buffer, "byteLength")
            .ok()
            .as_ref()
            .and_then(js::count)
            .ok_or_else(|| errors::backend("the memory's buffer has no length"))
    }

    fn memory_grow(&mut self, memory: Memory, pages: u64) -> Result<u64> {
        let object = self.objects.memory(memory)?;
        let delta = js::address(pages, object.ty.is_64());
        js::call_method(&object.memory, "grow", &[delta])
            .ok()
            .as_ref()
            .and_then(js::count)
            .ok_or(Error::Grow { delta: pages })
    }

    fn memory_read(&self, memory: Memory, offset: u64, buffer: &mut [u8]) -> Result<()> {
        self.memory_view(memory, offset, buffer.len() as u64)?
            .copy_to(buffer);
        Ok(())
    }

    fn memory_write(&mut self, memory: Memory, offset: u64, bytes: &[u8]) -> Result<()> {
        self.memory_view(memory, offset, bytes.len() as u64)?
            .copy_from(bytes);
        Ok(())
    }

    fn memory_with_bytes(
        &self,
        memory: Memory,
        offset: u64,
        len: usize,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<()> {
        // The browser cannot lend guest bytes to Rust, so the range is
        // copied once, and the copy is lent.
        let mut bytes = vec![0; len];
        self.memory_read(memory, offset, &mut bytes)?;
        f(&bytes);
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
        let source = self.memory_view(source, source_offset, len)?;
        let destination = self.memory_view(destination, destination_offset, len)?;
        // `TypedArray.set` copies as if through a buffer where the two
        // views overlap, so no buffer of the host is needed.
        destination.set(&source, 0);
        Ok(())
    }

    fn global_new(&mut self, ty: GlobalType, value: Val) -> Result<Global> {
        let content = ty.content();
        let name = value_type_name(content).ok_or_else(|| {
            errors::backend(format!(
                "the JavaScript API makes no global of type {content}"
            ))
        })?;
        values::check(&value, content, &self.types)?;
        let value = values::to_js(&self.objects, &value)?;
        let descriptor = js::object(&[
            ("value", JsValue::from_str(name)),
            (
                "mutable",
                JsValue::from_bool(ty.mutability() == Mutability::Var),
            ),
        ])
        .map_err(|error| errors::call(&error))?;
        let global =
            WebAssembly::Global::new(&descriptor, &value).map_err(|error| errors::call(&error))?;
        Ok(self.objects.add_global(GlobalObject { global, ty }))
    }

    fn global_ty(&self, global: Global) -> Result<GlobalType> {
        Ok(self.objects.global(global)?.ty)
    }

    fn global_get(&mut self, global: Global) -> Result<Val> {
        let object = self.objects.global(global)?;
        let kind = self.kind(object.ty.content());
        let value = js::get(&object.global, "value").map_err(|error| errors::call(&error))?;
        values::from_js(&mut self.objects, value, kind)
    }

    fn global_set(&mut self, global: Global, value: Val) -> Result<()> {
        let object = self.objects.global(global)?;
        if object.ty.mutability() == Mutability::Const {
            return Err(values::mismatch("the global is immutable".to_string()));
        }
        values::check(&value, object.ty.content(), &self.types)?;
        let value = values::to_js(&self.objects, &value)?;
        js::set(&object.global, "value", &value).map_err(|error| errors::call(&error))
    }

    fn table_new(&mut self, ty: TableType, init: Val) -> Result<Table> {
        let element = ValType::Ref(*ty.element());
        let name = table_element_name(ty.element().heap)
            .filter(|_| ty.element().nullable)
            .ok_or_else(|| {
                errors::backend(format!("the JavaScript API makes no table of {element}"))
            })?;
        values::check(&init, &element, &self.types)?;
        let init = values::to_js(&self.objects, &init)?;
        let is_64 = ty.is_64();
        let mut descriptor = vec![
            ("element", JsValue::from_str(name)),
            ("initial", js::address(ty.minimum(), is_64)),
        ];
        if let Some(maximum) = ty.maximum() {
            descriptor.push(("maximum", js::address(maximum, is_64)));
        }
        if is_64 {
            descriptor.push(("address", JsValue::from_str("i64")));
        }
        let descriptor = js::object(&descriptor).map_err(|error| errors::call(&error))?;
        let table = WebAssembly::Table::new_with_value(&descriptor, init)
            .map_err(|error| errors::call(&error))?;
        Ok(self.objects.add_table(TableObject { table, ty }))
    }

    fn table_ty(&self, table: Table) -> Result<TableType> {
        Ok(self.objects.table(table)?.ty)
    }

    fn table_size(&self, table: Table) -> Result<u64> {
        js::get(&self.objects.table(table)?.table, "length")
            .ok()
            .as_ref()
            .and_then(js::count)
            .ok_or_else(|| errors::backend("the table has no length"))
    }

    fn table_get(&mut self, table: Table, index: u64) -> Result<Val> {
        let size = self.table_size(table)?;
        if index >= size {
            return Err(Error::TableOutOfBounds { index, size });
        }
        let object = self.objects.table(table)?;
        let kind = self.kind(&ValType::Ref(*object.ty.element()));
        let index = js::address(index, object.ty.is_64());
        let value = js::call_method(&object.table, "get", &[index])
            .map_err(|error| errors::call(&error))?;
        values::from_js(&mut self.objects, value, kind)
    }

    fn table_set(&mut self, table: Table, index: u64, value: Val) -> Result<()> {
        let size = self.table_size(table)?;
        if index >= size {
            return Err(Error::TableOutOfBounds { index, size });
        }
        let object = self.objects.table(table)?;
        values::check(&value, &ValType::Ref(*object.ty.element()), &self.types)?;
        let value = values::to_js(&self.objects, &value)?;
        let index = js::address(index, object.ty.is_64());
        js::call_method(&object.table, "set", &[index, value])
            .map(|_| ())
            .map_err(|error| errors::call(&error))
    }

    fn table_grow(&mut self, table: Table, delta: u64, init: Val) -> Result<u64> {
        let object = self.objects.table(table)?;
        values::check(&init, &ValType::Ref(*object.ty.element()), &self.types)?;
        let init = values::to_js(&self.objects, &init)?;
        let amount = js::address(delta, object.ty.is_64());
        js::call_method(&object.table, "grow", &[amount, init])
            .ok()
            .as_ref()
            .and_then(js::count)
            .ok_or(Error::Grow { delta })
    }

    fn tag_ty(&self, tag: Tag) -> Result<TagType> {
        Ok(self.objects.tag(tag)?.ty.clone())
    }

    fn extern_ref_new(&mut self, value: Box<dyn Any + Send + Sync>) -> Result<ExternRef> {
        Ok(self.objects.add_extern_ref(value))
    }

    fn extern_ref_data(&self, extern_ref: ExternRef) -> Result<&(dyn Any + Send + Sync)> {
        self.objects
            .extern_ref(extern_ref)?
            .data
            .as_deref()
            .ok_or_else(|| {
                errors::backend("the externref holds a value that the host did not make")
            })
    }

    fn any_ref_from_i31(&mut self, value: I31) -> Result<AnyRef> {
        // The JavaScript API carries an `i31ref` as a `Number`, and makes an
        // `i31ref` of a `Number` in its range where a guest takes one.
        Ok(self
            .objects
            .add_any_ref(JsValue::from_f64(f64::from(value.get_i32()))))
    }

    fn any_ref_as_i31(&self, any_ref: AnyRef) -> Result<Option<I31>> {
        let value = self.objects.any_ref(any_ref)?.as_f64();
        Ok(value
            .filter(|value| {
                value.fract() == 0.0
                    && (-f64::from(1u32 << 30)..f64::from(1u32 << 31)).contains(value)
            })
            .map(|value| I31::wrapping_i32(value as i64 as i32)))
    }
}

/// The name of the kind of an extern of type `ty`.
fn extern_kind(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "a function",
        ExternType::Global(_) => "a global",
        ExternType::Table(_) => "a table",
        ExternType::Memory(_) => "a memory",
        ExternType::Tag(_) => "a tag",
    }
}

/// The name of the kind of `external`.
fn kind_of_extern(external: &Extern) -> &'static str {
    match external {
        Extern::Func(_) => "a function",
        Extern::Global(_) => "a global",
        Extern::Table(_) => "a table",
        Extern::Memory(_) => "a memory",
        Extern::Tag(_) => "a tag",
    }
}

/// The name the JavaScript API gives the value type `ty` in the descriptor
/// of a global, where it has one.
fn value_type_name(ty: &ValType) -> Option<&'static str> {
    match ty {
        ValType::I32 => Some("i32"),
        ValType::I64 => Some("i64"),
        ValType::F32 => Some("f32"),
        ValType::F64 => Some("f64"),
        ValType::V128 => None,
        ValType::Ref(ty) if ty.nullable => table_element_name(ty.heap),
        ValType::Ref(_) => None,
    }
}

/// The name the JavaScript API gives a nullable reference to `heap`, where
/// it has one.
fn table_element_name(heap: HeapType) -> Option<&'static str> {
    match heap {
        HeapType::Func => Some("anyfunc"),
        HeapType::Extern => Some("externref"),
        HeapType::Any => Some("anyref"),
        HeapType::Eq => Some("eqref"),
        HeapType::I31 => Some("i31ref"),
        HeapType::Struct => Some("structref"),
        HeapType::Array => Some("arrayref"),
        HeapType::None => Some("nullref"),
        HeapType::NoExtern => Some("nullexternref"),
        HeapType::NoFunc => Some("nullfuncref"),
        HeapType::Exn
        | HeapType::NoExn
        | HeapType::Cont
        | HeapType::NoCont
        | HeapType::Concrete(_) => None,
    }
}
